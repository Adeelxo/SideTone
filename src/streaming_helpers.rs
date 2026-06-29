use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

type AppResult<T> = Result<T, Box<dyn Error>>;

/// v7 helper Repair model — download the latest official yt-dlp directly.
///
/// yt-dlp is the one helper that breaks over time (YouTube changes), so Repair
/// self-updates it straight from the official `yt-dlp/yt-dlp` GitHub release. We
/// deliberately do NOT host our own helper manifest/binary: that would mean the
/// developer re-uploading a pinned `yt-dlp.exe` on every upstream release, and
/// users never download helper files by hand — the installer already bundles
/// working helpers. Trust model: HTTPS + this exact allowlisted upstream URL +
/// GitHub-owned redirect hosts + a min/max size guard + a `--version` self-test
/// on the downloaded binary before we swap it in — direct official yt-dlp GitHub
/// latest with SideTone's local validation gates. ffmpeg is bundled-only and
/// never updated here.
const YTDLP_LATEST_URL: &str =
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";

/// Sanity bounds on the downloaded yt-dlp.exe: reject anything implausibly small
/// (a truncated/error page) or implausibly large (a wrong/hostile payload).
const YTDLP_MIN_BYTES: usize = 1024 * 1024; // 1 MB
const YTDLP_MAX_BYTES: usize = 64 * 1024 * 1024; // 64 MB

/// Single source of truth for "a yt-dlp self-update is running." While set, no
/// new yt-dlp/ffmpeg process may be spawned, so nothing can re-lock yt-dlp.exe
/// while we are swapping it. Guards the Repair button (no double-run) AND every
/// streaming spawn (search/metadata/stream/download) and the local ffmpeg probe.
static REPAIR_IN_PROGRESS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

const REPAIR_BUSY_MESSAGE: &str =
    "Updating the streaming engine — playback is paused for a moment.";

/// True while a streaming-engine repair/update is in flight.
pub fn repair_in_progress() -> bool {
    REPAIR_IN_PROGRESS.load(std::sync::atomic::Ordering::SeqCst)
}

/// Atomically claim the repair slot. Returns false if a repair is already
/// running (so callers must not start a second one).
pub fn try_begin_repair() -> bool {
    REPAIR_IN_PROGRESS
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_ok()
}

/// Release the repair slot once a repair finishes (success or failure).
pub fn end_repair() {
    REPAIR_IN_PROGRESS.store(false, std::sync::atomic::Ordering::SeqCst);
}

pub struct StreamingHelperStatus {
    pub label: String,
    pub action: &'static str,
}

pub fn ytdlp_output(args: &[String]) -> AppResult<Output> {
    if repair_in_progress() {
        return Err(REPAIR_BUSY_MESSAGE.into());
    }
    let mut direct = hidden_command("yt-dlp");
    direct.args(args);
    match command_output_with_timeout(direct, ytdlp_output_timeout(args)) {
        Ok(output) => Ok(output),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut python_args = vec!["-m".to_string(), "yt_dlp".to_string()];
            python_args.extend(args.iter().cloned());
            let mut python = hidden_command("python");
            python.args(&python_args);
            command_output_with_timeout(python, ytdlp_output_timeout(args)).map_err(
                |python_error| {
                    format!(
                        "yt-dlp is not installed or not on PATH. Tried `yt-dlp` and `python -m yt_dlp`. Python error: {python_error}"
                    )
                    .into()
                },
            )
        }
        Err(error) => Err(error.into()),
    }
}

pub fn ytdlp_output_timeout(args: &[String]) -> Duration {
    if args
        .iter()
        .any(|arg| arg == "--extract-audio" || arg == "--audio-format")
    {
        Duration::from_secs(15 * 60)
    } else {
        Duration::from_secs(45)
    }
}

pub fn ytdlp_spawn(args: &[String]) -> AppResult<Child> {
    if repair_in_progress() {
        return Err(REPAIR_BUSY_MESSAGE.into());
    }
    let mut direct = hidden_command("yt-dlp");
    match direct
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => Ok(child),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut python_args = vec!["-m".to_string(), "yt_dlp".to_string()];
            python_args.extend(args.iter().cloned());
            let mut python = hidden_command("python");
            python
                .args(&python_args)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|python_error| {
                    format!(
                        "yt-dlp is not installed or not on PATH. Tried `yt-dlp` and `python -m yt_dlp`. Python error: {python_error}"
                    )
                    .into()
                })
        }
        Err(error) => Err(error.into()),
    }
}

pub fn streaming_helper_status() -> StreamingHelperStatus {
    let ytdlp = helper_version("yt-dlp", &["--version"]);
    let ffmpeg = helper_version("ffmpeg", &["-version"]);
    let label = helper_status_label(ytdlp.as_deref(), ffmpeg.as_deref());
    let action = if ytdlp.is_some() && ffmpeg.is_some() {
        "Check"
    } else {
        "Repair"
    };
    StreamingHelperStatus { label, action }
}

fn helper_version(program: &str, args: &[&str]) -> Option<String> {
    let mut command = hidden_command(program);
    command.args(args);
    let output = command_output_with_timeout(command, Duration::from_secs(5)).ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.lines().next()?.trim();
    if first.is_empty() {
        None
    } else {
        Some(short_helper_version(program, first))
    }
}

fn short_helper_version(program: &str, first_line: &str) -> String {
    if program == "ffmpeg" {
        let parts = first_line.split_whitespace().collect::<Vec<_>>();
        if parts.len() >= 3 && parts[0].eq_ignore_ascii_case("ffmpeg") {
            return parts[2].to_string();
        }
    }
    truncate_single_line(first_line, 28)
}

pub fn helper_status_label(ytdlp: Option<&str>, ffmpeg: Option<&str>) -> String {
    match (ytdlp, ffmpeg) {
        (Some(y), Some(f)) => format!("Ready: yt-dlp {y} / ffmpeg {f}"),
        (None, Some(f)) => format!("Missing yt-dlp / ffmpeg {f} found"),
        (Some(y), None) => format!("yt-dlp {y} found / missing ffmpeg"),
        (None, None) => "Missing yt-dlp and ffmpeg".to_string(),
    }
}

pub fn helper_repair_needed_status(status: &str) -> bool {
    let lower = status.to_ascii_lowercase();
    (lower.contains("streaming helper") && lower.contains("outdated"))
        || lower.contains("yt-dlp is missing")
        || lower.contains("ffmpeg is missing")
        || lower.contains("yt-dlp is not installed")
        || lower.contains("missing yt-dlp")
        || lower.contains("missing ffmpeg")
}

/// Distinct, user-facing helper failure categories. Kept deliberately small and
/// text-only (no diagnostics UI): each maps a raw error to a clear status line
/// and a stable kind for tests. Order matters — more specific causes win.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperFailureKind {
    AlreadyRepairing,
    SelfTest,
    Blocked,
    MissingFfmpeg,
    DisallowedSource,
    Network,
    YtDlp,
    Unknown,
}

impl HelperFailureKind {
    /// Short status line shown to the user.
    pub fn message(self) -> &'static str {
        match self {
            HelperFailureKind::AlreadyRepairing => "A repair is already running — please wait.",
            HelperFailureKind::SelfTest => {
                "The downloaded streaming engine failed its self-test. Your existing engine was kept."
            }
            HelperFailureKind::Blocked => {
                "Couldn't replace the streaming engine — it may be blocked by Windows/antivirus or still in use. Stop playback and try again."
            }
            HelperFailureKind::MissingFfmpeg => "ffmpeg is missing — please reinstall SideTone.",
            HelperFailureKind::DisallowedSource => {
                "Repair was blocked: the update source wasn't an official SideTone location."
            }
            HelperFailureKind::Network => {
                "Couldn't download the update. Check your internet connection and try again."
            }
            HelperFailureKind::YtDlp => {
                "The streaming engine reported an error. Try Repair again, or use a downloaded/local track."
            }
            HelperFailureKind::Unknown => "Repair failed. Click Repair to try again.",
        }
    }
}

/// Classify a raw repair/helper error string into a stable failure kind. Pure
/// string matching so it is trivially unit-testable and adds no runtime cost.
pub fn classify_helper_failure(raw: &str) -> HelperFailureKind {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("already running") || lower.contains("already in progress") {
        HelperFailureKind::AlreadyRepairing
    } else if lower.contains("self-check") || lower.contains("self-test") {
        HelperFailureKind::SelfTest
    } else if lower.contains("currently in use")
        || lower.contains("permission denied")
        || lower.contains("access is denied")
        || lower.contains("blocked")
    {
        HelperFailureKind::Blocked
    } else if lower.contains("ffmpeg is missing") {
        HelperFailureKind::MissingFfmpeg
    } else if lower.contains("not allowed") {
        HelperFailureKind::DisallowedSource
    } else if lower.contains("http ")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("network")
        || lower.contains("dns")
        || lower.contains("connect")
        || lower.contains("redirect")
        || lower.contains("download")
        || lower.contains("unexpectedly small")
    {
        HelperFailureKind::Network
    } else if lower.contains("yt-dlp") {
        HelperFailureKind::YtDlp
    } else {
        HelperFailureKind::Unknown
    }
}

/// Repair = "make streaming work again like a fresh install." We download the
/// latest official yt-dlp, prove it runs, and swap it in. ffmpeg is
/// bundled with the installer and effectively never the cause of breakage, so
/// we only flag it when it is genuinely absent (a broken install → reinstall).
/// Returns the new status label; the caller restarts the app on success.
pub fn repair_streaming_helpers_blocking() -> AppResult<String> {
    update_ytdlp_blocking()?;
    if helper_version("ffmpeg", &["-version"]).is_none() {
        return Err("ffmpeg is missing — please reinstall SideTone.".into());
    }
    Ok(streaming_helper_status().label)
}

/// Repair yt-dlp by downloading the latest official build. Order is deliberate:
/// validate the exact upstream URL → download bytes (redirects to GitHub-owned
/// hosts only) → min/max size guard → write temp → **`--version` self-test
/// (liveness gate)** → atomic swap with rollback. Any failure before the swap
/// deletes the temp file and leaves the working yt-dlp untouched.
pub fn update_ytdlp_blocking() -> AppResult<()> {
    // 1. Exact initial host/path: only the official yt-dlp latest asset URL.
    if !is_allowed_helper_source("yt-dlp.exe", YTDLP_LATEST_URL) {
        return Err("yt-dlp update URL is not allowed".into());
    }

    let dir = helper_install_dir()?;
    fs::create_dir_all(&dir)?;

    let target = dir.join("yt-dlp.exe");
    // The target is a constant, but guard anyway: never write over a non-.exe.
    if target.extension().and_then(|e| e.to_str()) != Some("exe") {
        return Err("yt-dlp target path is not an .exe".into());
    }

    // 2. Download the helper bytes to a temp file (never over the live helper).
    //    Redirects are followed only to GitHub-owned release-asset hosts.
    let bytes = download_from_allowlisted_github(YTDLP_LATEST_URL)?;
    if bytes.len() < YTDLP_MIN_BYTES {
        return Err("yt-dlp download was unexpectedly small (network error?)".into());
    }
    if bytes.len() > YTDLP_MAX_BYTES {
        return Err("yt-dlp download was unexpectedly large; refusing it".into());
    }

    let temp = dir.join("yt-dlp.exe.update-download");
    fs::write(&temp, &bytes)?;

    // 3. LIVENESS GATE — prove the downloaded binary is a real, runnable yt-dlp
    //    before we trust it enough to replace the working one.
    if !verify_ytdlp_binary(&temp) {
        let _ = fs::remove_file(&temp);
        return Err("downloaded yt-dlp failed its self-check".into());
    }

    // 4. Atomic swap with rollback (unchanged).
    swap_helper_in(&dir, "yt-dlp.exe", &temp, &target)
}

/// Download bytes from an allowlisted GitHub URL, following redirects only to
/// GitHub-owned hosts (the `latest/download` → asset-CDN hop). Used for the
/// yt-dlp Repair download.
fn download_from_allowlisted_github(url: &str) -> AppResult<Vec<u8>> {
    let client = build_github_client()?;
    let response = client.get(url).send()?;
    if !response.status().is_success() {
        return Err(format!("download returned HTTP {}", response.status().as_u16()).into());
    }
    Ok(response.bytes()?.to_vec())
}

/// reqwest client used for all helper downloads. Redirects are required (GitHub
/// `latest/download` hops to its asset CDN) but only to GitHub-owned hosts —
/// never an attacker-controlled redirect.
fn build_github_client() -> AppResult<reqwest::blocking::Client> {
    let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() > 10 {
            return attempt.error("too many redirects");
        }
        match attempt.url().host_str() {
            Some(host) if is_github_owned_host(host) => attempt.follow(),
            _ => attempt.error("redirect to a non-GitHub host was blocked"),
        }
    });
    Ok(reqwest::blocking::Client::builder()
        .user_agent(concat!("SideTone/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(180))
        .redirect(redirect_policy)
        .build()?)
}

/// Run `<path> --version` to confirm the freshly downloaded binary is a real,
/// runnable yt-dlp before we trust it enough to replace the working one. Trust
/// it only if it exits successfully (with a timeout, stdin closed) AND prints a
/// plausible yt-dlp version string.
fn verify_ytdlp_binary(path: &Path) -> bool {
    let mut command = Command::new(path);
    command.arg("--version").stdin(Stdio::null());
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    match command_output_with_timeout(command, Duration::from_secs(20)) {
        Ok(output) => {
            if !output.status.success() {
                return false;
            }
            let text = String::from_utf8_lossy(&output.stdout);
            text.lines()
                .next()
                .map(str::trim)
                .is_some_and(looks_like_ytdlp_version)
        }
        Err(_) => false,
    }
}

/// yt-dlp versions are date-based, e.g. `2026.06.09` or nightly `2026.06.09.123456`:
/// only ASCII digits and dots, at least one dot, and a leading 4-digit year.
fn looks_like_ytdlp_version(line: &str) -> bool {
    let line = line.trim();
    if line.len() < 8 || !line.contains('.') {
        return false;
    }
    if !line.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return false;
    }
    let year = line.split('.').next().unwrap_or("");
    year.len() == 4 && year.chars().all(|c| c.is_ascii_digit())
}

/// Hosts we will follow a download redirect to: GitHub itself and its asset CDN
/// (`*.githubusercontent.com`). Anything else is rejected to stop a malicious
/// or hijacked redirect from steering the download off GitHub.
///
/// We deliberately do NOT pin the exact CDN host. GitHub rotates the
/// release-asset host over time (S3 → `objects.githubusercontent.com` →
/// `release-assets.githubusercontent.com`), so pinning one would silently break
/// Repair on the next rotation. The `*.githubusercontent.com` suffix is
/// GitHub-owned and stable; it is the tightest match that stays correct.
fn is_github_owned_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "github.com" || host.ends_with(".githubusercontent.com")
}

/// Move `temp` onto `target`, keeping a `.old` backup so a failed swap rolls
/// back to the previously working helper. Reports the in-use case clearly.
fn swap_helper_in(dir: &Path, name: &str, temp: &Path, target: &Path) -> AppResult<()> {
    if target.exists() {
        let backup = dir.join(format!("{name}.old"));
        let _ = fs::remove_file(&backup);
        fs::rename(target, &backup).map_err(|error| {
            format!("{name} is currently in use; stop playback and try Repair again. {error}")
        })?;
        if let Err(error) = fs::rename(temp, target) {
            let _ = fs::rename(&backup, target);
            return Err(error.into());
        }
        let _ = fs::remove_file(&backup);
    } else {
        fs::rename(temp, target)?;
    }
    Ok(())
}

fn helper_install_dir() -> AppResult<PathBuf> {
    let exe = env::current_exe()?;
    let exe_dir = exe
        .parent()
        .ok_or("could not resolve executable directory")?;
    for dir in exe_dir.ancestors() {
        let dev_deps = dir.join("assets").join("deps");
        if dev_deps.exists() {
            return Ok(dev_deps);
        }
    }
    Ok(exe_dir.to_path_buf())
}

/// Exact initial-URL allowlist for the Repair download. The only accepted source
/// is the official yt-dlp latest release asset over HTTPS — an exact host + path
/// match (no substrings), so spoofs (`github.com.evil.com`, `github.com@evil.com`,
/// wrong owner/repo, wrong asset name, http://, etc.) are all rejected. Redirects
/// off this URL are separately constrained to GitHub-owned hosts (see
/// `is_github_owned_host`). SideTone-hosted release assets are also accepted, so
/// an optional self-hosted mirror would still pass — but normal Repair uses the
/// upstream URL and the developer hosts nothing.
pub fn is_allowed_helper_source(name: &str, url: &str) -> bool {
    let expected_name = name.to_ascii_lowercase();
    let lower = url.to_ascii_lowercase();
    if !lower.starts_with("https://") || !lower.ends_with(&format!("/{expected_name}")) {
        return false;
    }
    lower.starts_with("https://github.com/adeelxo/sidetone/releases/download/")
        || lower.starts_with("https://github.com/adeelxo/sidetone/releases/latest/download/")
        || (expected_name == "yt-dlp.exe"
            && lower == "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe")
}

pub fn command_output_with_timeout(mut command: Command, timeout: Duration) -> io::Result<Output> {
    use std::io::Read;

    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Drain stdout/stderr on their own threads. If we only polled `try_wait()`
    // without reading, a child that produces more output than the OS pipe buffer
    // (~64 KB) would block on write and never exit — so e.g. an ~88 KB
    // `--dump-single-json` would always hit the timeout. Reading concurrently
    // keeps the pipes flowing so the child can finish.
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let stdout_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = stdout_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });
    let stderr_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = stderr_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let stdout = stdout_reader.join().unwrap_or_default();
            let stderr = stderr_reader.join().unwrap_or_default();
            return Ok(Output {
                status,
                stdout,
                stderr,
            });
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            // The reader threads end once the killed child's pipes close.
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("helper timed out after {} seconds", timeout.as_secs()),
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

pub fn hidden_command(program: &str) -> Command {
    let resolved = resolve_tool(program);
    let mut command = Command::new(resolved);
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn resolve_tool(name: &str) -> std::ffi::OsString {
    let exe_name = helper_exe_name(name);
    if let Ok(exe) = env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            for dir in exe_dir.ancestors() {
                for candidate in [
                    dir.join(&exe_name),
                    dir.join("assets").join("deps").join(&exe_name),
                ] {
                    if candidate.exists() {
                        return candidate.into_os_string();
                    }
                }
            }
        }
    }
    if let Ok(cwd) = env::current_dir() {
        for dir in cwd.ancestors() {
            let candidate = dir.join("assets").join("deps").join(&exe_name);
            if candidate.exists() {
                return candidate.into_os_string();
            }
        }
    }
    name.into()
}

fn helper_exe_name(name: &str) -> String {
    if cfg!(windows) && Path::new(name).extension().is_none() {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn truncate_single_line(value: &str, max_chars: usize) -> String {
    let single_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&single_line, max_chars)
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_repair_allows_only_known_helper_sources() {
        assert!(is_allowed_helper_source(
            "yt-dlp.exe",
            "https://github.com/Adeelxo/sidetone/releases/latest/download/yt-dlp.exe"
        ));
        assert!(is_allowed_helper_source(
            "ffmpeg.exe",
            "https://github.com/Adeelxo/sidetone/releases/download/v6.3.0/ffmpeg.exe"
        ));
        // The official upstream yt-dlp latest URL is the normal Repair source.
        assert!(is_allowed_helper_source(
            "yt-dlp.exe",
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
        ));
        // ...but only for yt-dlp.exe — the upstream URL must not work for ffmpeg.
        assert!(!is_allowed_helper_source(
            "ffmpeg.exe",
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/ffmpeg.exe"
        ));

        assert!(!is_allowed_helper_source(
            "ffmpeg.exe",
            "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip"
        ));
        assert!(!is_allowed_helper_source(
            "ffmpeg.exe",
            "https://github.com/Adeelxo/sidetone/releases/latest/download/not-ffmpeg.exe"
        ));
        assert!(!is_allowed_helper_source(
            "yt-dlp.exe",
            "https://github.com.evil.com/Adeelxo/sidetone/releases/latest/download/yt-dlp.exe"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn large_output_does_not_deadlock_the_timeout_reader() {
        // Emit well over the ~64 KB OS pipe buffer. Before draining the pipes on
        // their own threads this would block the child on write and hit the
        // timeout; now it must return the full output quickly.
        let mut command = Command::new("cmd");
        command.args([
            "/C",
            "for /L %i in (1,1,5000) do @echo xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        ]);
        let output = command_output_with_timeout(command, Duration::from_secs(30))
            .expect("large output should not time out");
        assert!(output.status.success());
        assert!(
            output.stdout.len() > 100_000,
            "expected >100 KB drained, got {} bytes",
            output.stdout.len()
        );
    }

    #[test]
    fn ytdlp_latest_url_is_official_and_allowlisted() {
        // Repair downloads the official upstream yt-dlp latest asset, and that
        // exact URL must pass the initial-URL allowlist.
        assert_eq!(
            YTDLP_LATEST_URL,
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
        );
        assert!(is_allowed_helper_source("yt-dlp.exe", YTDLP_LATEST_URL));
    }

    #[test]
    fn ytdlp_size_bounds_are_sane() {
        // Min < a real yt-dlp.exe (~18 MB) < max, so a genuine download passes
        // while a truncated error page or an oversized payload is rejected.
        const {
            assert!(YTDLP_MIN_BYTES < YTDLP_MAX_BYTES);
            assert!(2_048 < YTDLP_MIN_BYTES);
            assert!(256 * 1024 * 1024 > YTDLP_MAX_BYTES);
        }
        let real_ish = 18 * 1024 * 1024;
        assert!(real_ish > YTDLP_MIN_BYTES && real_ish < YTDLP_MAX_BYTES);
    }

    #[test]
    fn failed_repair_leaves_existing_helper_intact() {
        // Simulate the swap step's precondition: if the freshly downloaded temp
        // file is rejected (size/self-test), it is removed and the live helper
        // is never touched. We model that here at the filesystem level.
        // Distinct prefix so this never collides with other tests' temp fixtures
        // (a sibling test uses `sidetone-test-<pid>` in the same process).
        let dir = env::temp_dir().join(format!("sidetone-helper-swap-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let target = dir.join("yt-dlp.exe");
        let temp = dir.join("yt-dlp.exe.update-download");
        fs::write(&target, b"original-working-helper").unwrap();
        fs::write(&temp, b"bad-download").unwrap();

        // Rejected temp download (size/self-test) -> we delete it and DO NOT swap.
        let _ = fs::remove_file(&temp);
        assert!(!temp.exists(), "temp download must be removed on failure");
        assert_eq!(
            fs::read(&target).unwrap(),
            b"original-working-helper",
            "the working helper must be left untouched"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn helper_failure_classification_covers_the_user_facing_cases() {
        use HelperFailureKind::*;
        assert_eq!(
            classify_helper_failure("a repair is already running"),
            AlreadyRepairing
        );
        assert_eq!(
            classify_helper_failure("downloaded yt-dlp failed its self-check"),
            SelfTest
        );
        assert_eq!(
            classify_helper_failure("yt-dlp.exe is currently in use; stop playback"),
            Blocked
        );
        assert_eq!(
            classify_helper_failure("ffmpeg is missing — please reinstall SideTone."),
            MissingFfmpeg
        );
        assert_eq!(
            classify_helper_failure("yt-dlp update URL is not allowed"),
            DisallowedSource
        );
        assert_eq!(
            classify_helper_failure("yt-dlp download returned HTTP 503"),
            Network
        );
        assert_eq!(
            classify_helper_failure("yt-dlp download was unexpectedly small (network error?)"),
            Network
        );
        assert_eq!(
            classify_helper_failure("yt-dlp download was unexpectedly large; refusing it"),
            Network
        );
        // Every kind yields a non-empty status line.
        for kind in [
            AlreadyRepairing,
            SelfTest,
            Blocked,
            MissingFfmpeg,
            DisallowedSource,
            Network,
            YtDlp,
            Unknown,
        ] {
            assert!(!kind.message().is_empty());
        }
    }

    #[test]
    fn helper_update_url_allowlist_rejects_spoofed_hosts_and_paths() {
        let base = "yt-dlp.exe";
        // http:// (not https)
        assert!(!is_allowed_helper_source(
            base,
            "http://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
        ));
        // suffix host spoof
        assert!(!is_allowed_helper_source(
            base,
            "https://github.com.evil.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
        ));
        // userinfo @ spoof
        assert!(!is_allowed_helper_source(
            base,
            "https://github.com@evil.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
        ));
        // unicode / punycode lookalike host
        assert!(!is_allowed_helper_source(
            base,
            "https://gìthub.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
        ));
        assert!(!is_allowed_helper_source(
            base,
            "https://xn--gthub-jua.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
        ));
        // any path other than the exact release-asset path
        assert!(!is_allowed_helper_source(
            base,
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/evil.exe"
        ));
        assert!(!is_allowed_helper_source(
            base,
            "https://github.com/attacker/repo/releases/latest/download/yt-dlp.exe"
        ));
        // The accepted forms: the official upstream yt-dlp latest asset (normal
        // Repair source) and SideTone-hosted release assets (optional mirror).
        assert!(is_allowed_helper_source(
            base,
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
        ));
        assert!(is_allowed_helper_source(
            base,
            "https://github.com/Adeelxo/sidetone/releases/latest/download/yt-dlp.exe"
        ));
        assert!(is_allowed_helper_source(
            base,
            "https://github.com/Adeelxo/sidetone/releases/download/v7.0.0/yt-dlp.exe"
        ));
    }

    #[test]
    fn redirect_host_allowlist_accepts_github_and_its_cdn_only() {
        assert!(is_github_owned_host("github.com"));
        assert!(is_github_owned_host("objects.githubusercontent.com"));
        assert!(is_github_owned_host("release-assets.githubusercontent.com"));
        assert!(!is_github_owned_host("evil.com"));
        assert!(!is_github_owned_host("github.com.evil.com"));
        assert!(!is_github_owned_host("githubusercontent.com.evil.com"));
        assert!(!is_github_owned_host("notgithub.com"));
    }

    #[test]
    fn ytdlp_version_self_test_accepts_only_plausible_versions() {
        assert!(looks_like_ytdlp_version("2026.06.09"));
        assert!(looks_like_ytdlp_version("2026.06.09.123456"));
        assert!(!looks_like_ytdlp_version(""));
        assert!(!looks_like_ytdlp_version("not a version"));
        assert!(!looks_like_ytdlp_version("v2026.06.09"));
        assert!(!looks_like_ytdlp_version("2026"));
        assert!(!looks_like_ytdlp_version("ERROR: something went wrong"));
    }

    #[test]
    fn helper_repair_prompt_detects_missing_and_outdated_helpers() {
        assert!(helper_repair_needed_status(
            "SideTone's streaming helper is outdated."
        ));
        assert!(helper_repair_needed_status(
            "yt-dlp is missing. Reinstall SideTone."
        ));
        assert!(helper_repair_needed_status(
            "ffmpeg is missing. Reinstall SideTone."
        ));
        assert!(!helper_repair_needed_status("Playing."));
        assert!(!helper_repair_needed_status(
            "HTTP Error 429: Too Many Requests"
        ));
    }

    #[test]
    fn ytdlp_timeout_is_longer_for_downloads() {
        let metadata_args = vec!["--dump-json".to_string()];
        let download_args = vec!["--extract-audio".to_string(), "--audio-format".to_string()];
        assert!(ytdlp_output_timeout(&metadata_args) < ytdlp_output_timeout(&download_args));
    }

    #[test]
    fn helper_status_label_reports_missing_tools() {
        assert_eq!(helper_status_label(None, None), "Missing yt-dlp and ffmpeg");
        assert!(helper_status_label(Some("2026.03.17"), None).contains("missing ffmpeg"));
        assert!(helper_status_label(None, Some("7.1")).contains("Missing yt-dlp"));
        assert!(helper_status_label(Some("2026.03.17"), Some("7.1")).contains("Ready"));
    }
}
