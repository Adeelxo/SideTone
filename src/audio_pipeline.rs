//! Audio pipeline: yt-dlp/ffmpeg process lifecycle for live streaming + buffered
//! download, the YoutubeStreamSource (owns both child handles; Drop kills+reaps),
//! download-in-flight dedupe, favorite downloads, and stale temp-dir sweeping.
//! Extracted verbatim from `main.rs` (move-only; no logic changes).
//!
//! Ownership notes (see P2 scope): YoutubeStreamSource owns the ytdlp+ffmpeg
//! children; the pipe thread only holds the stdout/stdin pipe ends. The repair
//! gate blocks both stream and download at ytdlp_spawn/ytdlp_output. The buffered
//! download's temp dir is held by a `ScopedTempDir` RAII guard that removes it on
//! every failure path and is disarmed via `keep()` on success (the caller then
//! owns teardown). cleanup_temp_dir stays in main as a shared util so player.rs
//! needs no dependency on this module.

use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::num::{NonZeroU16, NonZeroU32};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rodio::Source;

use crate::downloads::{favorite_audio_files, favorites_dir, newest_favorite_file};
use crate::streaming::{ensure_success, yt_target};
use crate::streaming_helpers::{hidden_command, ytdlp_output, ytdlp_spawn};
use crate::{cleanup_temp_dir, format_duration, timestamp_millis, yt_metadata, AppResult};

pub(crate) fn yt_play_buffered(input: &str) -> AppResult<String> {
    let (title, audio_file, temp_dir) = prepare_youtube_audio(input)?;

    let mut stream_handle = rodio::DeviceSinkBuilder::open_default_sink()?;
    stream_handle.log_on_drop(false);
    let player = rodio::Player::connect_new(stream_handle.mixer());
    let file = File::open(&audio_file)?;
    player.append(rodio::Decoder::try_from(file)?);
    player.sleep_until_end();
    cleanup_temp_dir(&temp_dir);

    Ok(title)
}

pub(crate) fn prepare_youtube_stream(
    input: &str,
) -> AppResult<(String, Option<Duration>, String, YoutubeStreamSource)> {
    let target = yt_target(input);
    let metadata = yt_metadata(&target)?;
    let title = metadata.title.as_deref().unwrap_or("Untitled").to_string();
    let duration = metadata.duration.map(Duration::from_secs_f64);
    let stream = ytdlp_stream_audio(&target, None)?;
    Ok((title, duration, target, stream))
}

fn prepare_youtube_audio(input: &str) -> AppResult<(String, PathBuf, PathBuf)> {
    let target = yt_target(input);
    println!("Resolving: {input}");

    let metadata = yt_metadata(&target)?;
    let title = metadata.title.as_deref().unwrap_or("Untitled");
    if let Some(duration) = metadata.duration {
        if duration > 20.0 * 60.0 {
            return Err(format!(
                "refusing to buffer {title} because it is {} long. Pick a shorter track for this first buffered prototype.",
                format_duration(duration)
            )
            .into());
        }
    }

    println!("Playing: {title}");
    println!("Downloading with yt-dlp...");
    let (temp_dir, audio_file) = ytdlp_download_audio(&target)?;

    Ok((title.to_string(), audio_file, temp_dir))
}

pub(crate) struct YoutubeStreamSource {
    reader: BufReader<std::process::ChildStdout>,
    ytdlp: Child,
    ffmpeg: Child,
    pipe_thread: Option<JoinHandle<()>>,
}

impl Iterator for YoutubeStreamSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let mut bytes = [0u8; 4];
        match self.reader.read_exact(&mut bytes) {
            Ok(()) => Some(f32::from_le_bytes(bytes)),
            Err(_) => None,
        }
    }
}

impl Source for YoutubeStreamSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> rodio::ChannelCount {
        NonZeroU16::new(2).expect("stream channel count is non-zero")
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        NonZeroU32::new(48_000).expect("stream sample rate is non-zero")
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

impl Drop for YoutubeStreamSource {
    fn drop(&mut self) {
        let _ = self.ytdlp.kill();
        let _ = self.ffmpeg.kill();
        let _ = self.ytdlp.wait();
        let _ = self.ffmpeg.wait();
        if let Some(pipe_thread) = self.pipe_thread.take() {
            let _ = pipe_thread.join();
        }
    }
}

fn kill_and_wait(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

pub(crate) fn ytdlp_stream_audio(
    target: &str,
    start_at: Option<Duration>,
) -> AppResult<YoutubeStreamSource> {
    let args = vec![
        "--no-playlist".to_string(),
        "-f".to_string(),
        "bestaudio".to_string(),
        "-o".to_string(),
        "-".to_string(),
        "--no-warnings".to_string(),
        target.to_string(),
    ];
    let mut ytdlp = ytdlp_spawn(&args)?;
    let mut ytdlp_stdout = match ytdlp.stdout.take() {
        Some(stdout) => stdout,
        None => {
            kill_and_wait(&mut ytdlp);
            return Err("yt-dlp stream stdout was not available".into());
        }
    };

    let mut ffmpeg_args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-i".to_string(),
        "pipe:0".to_string(),
    ];
    if let Some(start_at) = start_at.filter(|duration| !duration.is_zero()) {
        ffmpeg_args.push("-ss".to_string());
        ffmpeg_args.push(format!("{:.3}", start_at.as_secs_f64()));
    }
    ffmpeg_args.extend([
        "-f".to_string(),
        "f32le".to_string(),
        "-acodec".to_string(),
        "pcm_f32le".to_string(),
        "-ac".to_string(),
        "2".to_string(),
        "-ar".to_string(),
        "48000".to_string(),
        "pipe:1".to_string(),
    ]);

    let mut ffmpeg = match hidden_command("ffmpeg")
        .args(ffmpeg_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            kill_and_wait(&mut ytdlp);
            return Err(error.into());
        }
    };

    let mut ffmpeg_stdin = match ffmpeg.stdin.take() {
        Some(stdin) => stdin,
        None => {
            kill_and_wait(&mut ytdlp);
            kill_and_wait(&mut ffmpeg);
            return Err("ffmpeg stdin was not available".into());
        }
    };
    let ffmpeg_stdout = match ffmpeg.stdout.take() {
        Some(stdout) => stdout,
        None => {
            kill_and_wait(&mut ytdlp);
            kill_and_wait(&mut ffmpeg);
            return Err("ffmpeg stdout was not available".into());
        }
    };

    let pipe_thread = thread::spawn(move || {
        let _ = io::copy(&mut ytdlp_stdout, &mut ffmpeg_stdin);
    });

    Ok(YoutubeStreamSource {
        reader: BufReader::new(ffmpeg_stdout),
        ytdlp,
        ffmpeg,
        pipe_thread: Some(pipe_thread),
    })
}

/// RAII scratch directory: removes itself on drop unless `keep()` is called. The
/// buffered download below creates a temp dir up front, then has several `?`
/// failure paths before it succeeds; the guard makes each of those (and any
/// future early return, including a panic unwind) clean up the dir automatically.
/// On success the caller takes ownership via `keep()` and is then responsible for
/// teardown after playback (`cleanup_temp_dir`).
struct ScopedTempDir {
    path: PathBuf,
    armed: bool,
}

impl ScopedTempDir {
    fn new(path: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&path)?;
        Ok(Self { path, armed: true })
    }

    /// Disarm the guard and hand the directory to the caller (success path).
    fn keep(mut self) -> PathBuf {
        self.armed = false;
        std::mem::take(&mut self.path)
    }
}

impl Drop for ScopedTempDir {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn ytdlp_download_audio(target: &str) -> AppResult<(PathBuf, PathBuf)> {
    let temp = ScopedTempDir::new(env::temp_dir().join(format!(
        "sidetone-{}-{}",
        std::process::id(),
        timestamp_millis()
    )))?;

    let output_template = temp.path.join("%(id)s.%(ext)s");
    let args = vec![
        "--no-playlist".to_string(),
        "-f".to_string(),
        "bestaudio".to_string(),
        "--extract-audio".to_string(),
        "--audio-format".to_string(),
        "mp3".to_string(),
        "--audio-quality".to_string(),
        "0".to_string(),
        "--no-warnings".to_string(),
        "-o".to_string(),
        output_template.to_string_lossy().to_string(),
        target.to_string(),
    ];
    let output = ytdlp_output(&args)?;
    ensure_success("yt-dlp download", &output)?;

    let audio_file = fs::read_dir(&temp.path)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "yt-dlp did not produce an audio file in {}",
                temp.path.display()
            )
        })?;

    // Success: hand the (still-present) temp dir to the caller, who cleans it up
    // after playback. Any failure above dropped the armed guard and removed it.
    Ok((temp.keep(), audio_file))
}

/// Tracks currently being downloaded, keyed by source URL. Prevents two
/// concurrent yt-dlp downloads of the same track (e.g. a double-click, or the
/// same URL selected in a batch) from racing to write the same output file.
fn downloads_in_flight() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// RAII claim on a download URL; releases on drop so a failed/finished download
/// frees the slot automatically.
struct DownloadGuard(String);

impl DownloadGuard {
    fn try_acquire(url: &str) -> Option<DownloadGuard> {
        let mut set = downloads_in_flight().lock().ok()?;
        if !set.insert(url.to_string()) {
            return None; // already downloading this exact track
        }
        Some(DownloadGuard(url.to_string()))
    }
}

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = downloads_in_flight().lock() {
            set.remove(&self.0);
        }
    }
}

pub(crate) fn save_favorite_track(target: &str) -> AppResult<PathBuf> {
    // Dedupe concurrent downloads of the same track. Held for the whole download.
    let _guard = DownloadGuard::try_acquire(target)
        .ok_or("This track is already downloading. Please wait.")?;
    let favorites = favorites_dir()?;
    fs::create_dir_all(&favorites)?;

    let before = favorite_audio_files()?;
    let output_template = favorites.join("%(title).80B [%(id)s].%(ext)s");
    let args = vec![
        "--no-playlist".to_string(),
        "--restrict-filenames".to_string(),
        "-f".to_string(),
        "bestaudio".to_string(),
        "--extract-audio".to_string(),
        "--audio-format".to_string(),
        "mp3".to_string(),
        "--audio-quality".to_string(),
        "5".to_string(),
        "--no-warnings".to_string(),
        "-o".to_string(),
        output_template.to_string_lossy().to_string(),
        target.to_string(),
    ];
    let output = ytdlp_output(&args)?;
    ensure_success("yt-dlp favorite download", &output)?;

    let after = favorite_audio_files()?;
    after
        .into_iter()
        .find(|path| !before.iter().any(|old| old == path))
        .or_else(|| newest_favorite_file().ok().flatten())
        .ok_or_else(|| "favorite download finished but no audio file was found".into())
}

/// Remove leftover `sidetone-<pid>-<ts>` buffered-audio folders from previous
/// runs. Only the download pattern (a numeric pid segment) is touched, and only
/// when the pid is not the current process — so test fixtures (`sidetone-test-*`,
/// `sidetone-scan-test-*`, etc., which start with a non-digit) are left alone.
pub(crate) fn sweep_stale_temp_dirs() {
    let current = std::process::id().to_string();
    let Ok(entries) = fs::read_dir(env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name();
        let Some(rest) = name.to_str().and_then(|n| n.strip_prefix("sidetone-")) else {
            continue;
        };
        let Some(pid) = rest.split('-').next() else {
            continue;
        };
        if pid.is_empty() || !pid.chars().all(|c| c.is_ascii_digit()) || pid == current {
            continue;
        }
        let _ = fs::remove_dir_all(entry.path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_temp_dir_removes_on_drop_when_armed() {
        let path =
            env::temp_dir().join(format!("sidetone-scopedtemp-armed-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        {
            let guard = ScopedTempDir::new(path.clone()).expect("create temp dir");
            assert!(guard.path.is_dir(), "new() must create the dir");
        } // armed guard dropped here
        assert!(!path.exists(), "an armed guard must remove the dir on drop");
    }

    #[test]
    fn scoped_temp_dir_keep_preserves_dir() {
        let path = env::temp_dir().join(format!("sidetone-scopedtemp-keep-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        let kept = {
            let guard = ScopedTempDir::new(path.clone()).expect("create temp dir");
            guard.keep()
        }; // disarmed guard dropped here
        assert_eq!(kept, path, "keep() returns the dir path");
        assert!(
            path.is_dir(),
            "a kept (disarmed) dir must persist after the guard drops"
        );
        let _ = fs::remove_dir_all(&path); // caller-owned cleanup (mirrors cleanup_temp_dir)
    }
}
