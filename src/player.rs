//! Playback engine (UI-free): the rodio PlayerController, its tune/focus/reverb
//! source-wrapper chain, per-track tune memory, duration probing, and the
//! progress/seek speed-scaling. Extracted verbatim from `main.rs` (move-only;
//! no logic changes). yt-dlp stream/download spawns and all playback
//! orchestration stay in `main.rs` (the later audio_pipeline split).

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rodio::Source;
use serde::{Deserialize, Serialize};

use crate::domain::QueuedTrack;
use crate::persistence::{data_dir, write_json_atomic};
use crate::streaming_helpers::{self, command_output_with_timeout, hidden_command};
use crate::{format_duration, open_output_sink, AppResult, OutputDeviceInfo};

// --- Per-track tune memory -------------------------------------------------
// Opt-in: only local/downloaded tracks are remembered, keyed by file path, in
// tune.json next to the executable. YouTube streams are never saved.

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) struct TuneSetting {
    pub(crate) speed: f32,
    pub(crate) reverb: f32,
}

fn tune_store_path() -> Option<PathBuf> {
    Some(data_dir()?.join("tune.json"))
}

fn load_tune_store() -> BTreeMap<String, TuneSetting> {
    tune_store_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_tune_store(store: &BTreeMap<String, TuneSetting>) {
    if let Some(path) = tune_store_path() {
        let _ = write_json_atomic(&path, store);
    }
}

pub(crate) fn load_tune_for(key: &str) -> Option<TuneSetting> {
    load_tune_store().get(key).copied()
}

pub(crate) fn save_tune_for(key: &str, setting: TuneSetting) {
    let mut store = load_tune_store();
    store.insert(key.to_string(), setting);
    save_tune_store(&store);
}

pub(crate) fn clear_tune_for(key: &str) {
    let mut store = load_tune_store();
    if store.remove(key).is_some() {
        save_tune_store(&store);
    }
}

pub(crate) struct PlayerController {
    _stream: rodio::MixerDeviceSink,
    mixer: rodio::mixer::Mixer,
    pub(crate) selected_output_index: usize,
    current: Option<Arc<rodio::Player>>,
    current_tracks: Vec<PathBuf>,
    current_duration: Option<Duration>,
    current_seekable: bool,
    pub(crate) current_stream_target: Option<String>,
    current_stream_title: Option<String>,
    current_stream_offset: Duration,
    focus_enabled: bool,
    focus_flag: Arc<AtomicBool>,
    focus_intensity: f32,
    focus_intensity_value: Arc<AtomicU32>,
    // TUNE: speed couples pitch+tempo (the "slowed" sound); reverb mix is read
    // live by the Reverb source wrapper. speed (main-thread copy) scales the
    // displayed/seek timeline; speed_factor is read live by the VarSpeed wrapper
    // on the audio thread so changes take effect mid-track.
    speed: f32,
    speed_factor: Arc<AtomicU32>,
    reverb_mix: Arc<AtomicU32>,
    repeat: RepeatMode,
    volume: f32,
    paused: bool,
    auto_advanced: bool,
    pub(crate) seek_generation: u64,
    // When the current track started. Used as a grace window so a stream that
    // fails to buffer (reports empty() immediately) isn't mistaken for a track
    // that ended â€” which used to cascade auto-advance through the whole queue
    // ("stuck in a loop"). None when nothing is loaded.
    started_at: Option<Instant>,
}

impl PlayerController {
    pub(crate) fn new(device: Option<cpal::Device>) -> AppResult<Self> {
        let mut stream = open_output_sink(device)?;
        stream.log_on_drop(false);
        let mixer = stream.mixer().clone();

        Ok(Self {
            _stream: stream,
            mixer,
            selected_output_index: 0,
            current: None,
            current_tracks: Vec::new(),
            current_duration: None,
            current_seekable: false,
            current_stream_target: None,
            current_stream_title: None,
            current_stream_offset: Duration::ZERO,
            focus_enabled: false,
            focus_flag: Arc::new(AtomicBool::new(false)),
            focus_intensity: 0.5,
            focus_intensity_value: Arc::new(AtomicU32::new(0.5f32.to_bits())),
            speed: 1.0,
            speed_factor: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            reverb_mix: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            repeat: RepeatMode::Off,
            volume: 0.64,
            paused: false,
            auto_advanced: false,
            seek_generation: 0,
            started_at: None,
        })
    }

    pub(crate) fn play_stream<S>(
        &mut self,
        source: S,
        duration: Option<Duration>,
        stream_target: String,
        stream_title: String,
    ) -> AppResult<()>
    where
        S: Source + Send + 'static,
    {
        self.stop();
        let player = self.new_player();
        player.append(self.tune_source(source));

        self.current = Some(player);
        self.current_tracks.clear();
        self.current_duration = duration;
        self.current_seekable = false;
        self.current_stream_target = Some(stream_target);
        self.current_stream_title = Some(stream_title);
        self.current_stream_offset = Duration::ZERO;
        self.paused = false;
        self.auto_advanced = false;
        self.seek_generation = self.seek_generation.wrapping_add(1);
        self.started_at = Some(Instant::now());
        Ok(())
    }

    pub(crate) fn play_files(
        &mut self,
        tracks: Vec<PathBuf>,
        // Precomputed by the caller BEFORE the controller lock is taken, so the
        // (potentially slow) ffmpeg duration probe never runs while holding the
        // lock â€” that used to stall the UI thread into "Not Responding".
        duration: Option<Duration>,
    ) -> AppResult<()> {
        if tracks.is_empty() {
            return Err("no tracks to play".into());
        }

        self.stop();
        let player = self.build_player(&tracks)?;
        let current_duration = duration;
        self.current = Some(player);
        self.current_tracks = tracks;
        self.current_duration = current_duration;
        self.current_seekable = true;
        self.current_stream_target = None;
        self.current_stream_title = None;
        self.current_stream_offset = Duration::ZERO;
        self.paused = false;
        self.auto_advanced = false;
        self.seek_generation = self.seek_generation.wrapping_add(1);
        self.started_at = Some(Instant::now());
        Ok(())
    }

    fn build_player(&self, tracks: &[PathBuf]) -> AppResult<Arc<rodio::Player>> {
        let player = self.new_player();

        for track in tracks {
            let file = File::open(track)?;
            let decoder = rodio::Decoder::try_from(file)?;
            player.append(self.tune_source(decoder));
        }

        Ok(player)
    }

    fn new_player(&self) -> Arc<rodio::Player> {
        let player = Arc::new(rodio::Player::connect_new(&self.mixer));
        player.set_volume(self.volume);
        // Speed is handled by the VarSpeed wrapper (constant output rate) so it
        // works live; the rodio Player stays at 1.0. See [[rodio-speed-semantics]].
        player
    }

    pub(crate) fn select_output_device(
        &mut self,
        index: usize,
        devices: &[OutputDeviceInfo],
    ) -> AppResult<usize> {
        if devices.is_empty() {
            return Ok(self.selected_output_index);
        }

        if index == self.selected_output_index {
            return Ok(self.selected_output_index);
        }

        self.stop();
        let mut stream = open_output_sink(devices[index].device.clone())?;
        stream.log_on_drop(false);
        self.mixer = stream.mixer().clone();
        self._stream = stream;
        self.selected_output_index = index;
        Ok(self.selected_output_index)
    }

    /// Wrap a source in the TUNE/Focus DSP chain: source -> speed -> reverb ->
    /// focus. All three read their controls from atomics, so the dial works live.
    fn tune_source<S>(&self, source: S) -> FocusMuffle<Reverb<VarSpeed<S>>>
    where
        S: Source,
    {
        let speed = VarSpeed::new(source, Arc::clone(&self.speed_factor));
        let reverb = Reverb::new(speed, Arc::clone(&self.reverb_mix));
        FocusMuffle::new(
            reverb,
            Arc::clone(&self.focus_flag),
            Arc::clone(&self.focus_intensity_value),
        )
    }

    /// Apply speed (couples pitch) + reverb mix. Both are read live on the audio
    /// thread via atomics; `self.speed` is the main-thread copy for timeline math.
    pub(crate) fn set_tune(&mut self, speed: f32, reverb: f32) {
        self.speed = speed.clamp(0.5, 1.5);
        self.speed_factor
            .store(self.speed.to_bits(), Ordering::Relaxed);
        self.reverb_mix
            .store(reverb.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn current_tune(&self) -> (f32, f32) {
        (
            self.speed,
            f32::from_bits(self.reverb_mix.load(Ordering::Relaxed)),
        )
    }

    /// Key for per-track tune memory: the local file path, or None for streams
    /// (YouTube) and when nothing local is loaded.
    pub(crate) fn current_tune_key(&self) -> Option<String> {
        if self.current_stream_target.is_some() {
            return None;
        }
        self.current_tracks
            .first()
            .map(|path| path.to_string_lossy().to_string())
    }

    pub(crate) fn toggle_pause(&mut self) -> bool {
        let Some(player) = &self.current else {
            return self.paused;
        };

        if player.is_paused() {
            player.play();
            self.paused = false;
        } else {
            player.pause();
            self.paused = true;
        }

        self.paused
    }

    pub(crate) fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(player) = &self.current {
            player.set_volume(self.volume);
        }
    }

    pub(crate) fn stop(&mut self) {
        if let Some(player) = &self.current {
            player.stop();
        }
        self.current = None;
        self.current_tracks.clear();
        self.current_duration = None;
        self.current_seekable = false;
        self.current_stream_target = None;
        self.current_stream_title = None;
        self.current_stream_offset = Duration::ZERO;
        self.paused = false;
        self.auto_advanced = false;
        self.seek_generation = self.seek_generation.wrapping_add(1);
        self.started_at = None;
    }

    pub(crate) fn progress_snapshot(&self) -> ProgressSnapshot {
        let Some(player) = &self.current else {
            return ProgressSnapshot::default();
        };

        // get_pos() is speed-scaled wall-clock; current_duration is source time,
        // so the playable wall-clock total is duration / speed.
        let position = player.get_pos() + self.current_stream_offset;
        let duration = self
            .current_duration
            .map(|d| playable_wall_total(d, self.speed))
            .unwrap_or_default();
        let percent = progress_percent(position, duration);

        ProgressSnapshot {
            percent,
            seekable: self.can_scrub(),
            label: format!(
                "{} / {}",
                format_duration(position.as_secs_f64()),
                format_duration(duration.as_secs_f64())
            ),
        }
    }

    // Fast path: local files seek in place. Returns None when this isn't a
    // local-seekable track, so the caller falls back to the stream-seek path.
    // This is cheap and safe to run on the UI thread.
    pub(crate) fn seek_local(&mut self, percent: f32) -> Option<ProgressSnapshot> {
        if !self.current_seekable {
            return None;
        }
        let duration = self.current_duration?;
        if duration.is_zero() {
            return Some(self.progress_snapshot());
        }
        let fraction = percent.clamp(0.0, 100.0) / 100.0;
        let target_wall = playable_wall_total(duration, self.speed).mul_f32(fraction);
        if let Some(player) = &self.current {
            let _ = player.try_seek(target_wall);
        }
        Some(self.progress_snapshot())
    }

    // Stream-seek step 1 (UI thread): compute the plan and stop the current
    // player. The slow yt-dlp/ffmpeg restart is resolved OFF the UI thread; the
    // caller then hands the resolved stream to `apply_stream_seek`. We set
    // `auto_advanced` so the progress timer doesn't mistake the stopped player
    // for a finished track and auto-advance during the gap.
    pub(crate) fn prepare_stream_seek(&mut self, percent: f32) -> Option<StreamSeekPlan> {
        if self.current_seekable {
            return None;
        }
        let duration = self.current_duration?;
        if duration.is_zero() {
            return None;
        }
        let stream_target = self.current_stream_target.clone()?;
        let fraction = percent.clamp(0.0, 100.0) / 100.0;
        let wall_total = playable_wall_total(duration, self.speed);
        let target_wall = wall_total.mul_f32(fraction);
        let source_target = duration.mul_f32(fraction);
        if let Some(player) = &self.current {
            player.stop();
        }
        self.auto_advanced = true;
        self.seek_generation = self.seek_generation.wrapping_add(1);
        let generation = self.seek_generation;
        Some(StreamSeekPlan {
            stream_target,
            source_target,
            target_wall,
            wall_total,
            percent: percent.clamp(0.0, 100.0),
            generation,
        })
    }

    // Stream-seek step 2 (after the resolve): swap in the restarted stream.
    pub(crate) fn apply_stream_seek<S>(&mut self, stream: S, plan: &StreamSeekPlan) -> bool
    where
        S: Source + Send + 'static,
    {
        if self.seek_generation != plan.generation
            || self.current_stream_target.as_deref() != Some(plan.stream_target.as_str())
        {
            return false;
        }
        let new_player = self.new_player();
        new_player.append(self.tune_source(stream));
        self.current = Some(new_player);
        self.current_stream_offset = plan.target_wall;
        self.paused = false;
        self.auto_advanced = false;
        true
    }

    fn can_scrub(&self) -> bool {
        self.current_seekable || self.current_stream_target.is_some()
    }

    fn current_stream_track(&self) -> Option<QueuedTrack> {
        Some(QueuedTrack {
            title: self
                .current_stream_title
                .clone()
                .unwrap_or_else(|| "YouTube".to_string()),
            url: self.current_stream_target.clone()?,
        })
    }

    pub(crate) fn take_playback_action(&mut self) -> PlaybackAction {
        if self.auto_advanced || self.paused {
            return PlaybackAction::None;
        }
        // Grace window: a freshly-started track (especially a stream still
        // buffering) can momentarily report empty(); don't treat that as "ended"
        // or we cascade-advance through the whole queue.
        if let Some(started) = self.started_at {
            if started.elapsed() < Duration::from_millis(1500) {
                return PlaybackAction::None;
            }
        }
        let Some(player) = &self.current else {
            return PlaybackAction::None;
        };
        // Compare in wall-clock: position is speed-scaled, so scale duration too.
        let duration = self
            .current_duration
            .unwrap_or_default()
            .div_f32(self.speed.max(0.01));
        let position = player.get_pos() + self.current_stream_offset;
        // Streams can stall just before `empty()` flips, so nudge them when the
        // known duration is nearly reached. Local files drain reliably â€” wait
        // for the sink to actually empty so the last second isn't clipped.
        let near_known_end = self.current_stream_target.is_some()
            && !duration.is_zero()
            && position >= duration.saturating_sub(Duration::from_millis(800));

        if !player.empty() && !near_known_end {
            return PlaybackAction::None;
        }

        self.auto_advanced = true;

        // Repeat-one: replay the very same track (stream or local file).
        if self.repeat == RepeatMode::One {
            if let Some(track) = self.current_stream_track() {
                return PlaybackAction::LoopStream(track);
            }
            if !self.current_tracks.is_empty() {
                return PlaybackAction::LoopLocal(self.current_tracks.clone());
            }
            return PlaybackAction::None;
        }

        // Off / All: advance to the next track. The event loop decides whether
        // to wrap (All) or stop (Off) when the queue runs out. Works for both
        // YouTube streams and local files.
        PlaybackAction::Advance
    }

    pub(crate) fn set_focus_enabled(&mut self, enabled: bool) -> AppResult<()> {
        self.focus_enabled = enabled;
        self.focus_flag.store(enabled, Ordering::Relaxed);
        Ok(())
    }

    pub(crate) fn set_focus_intensity(&mut self, intensity: f32) {
        self.focus_intensity = intensity.clamp(0.0, 1.0);
        self.focus_intensity_value
            .store(self.focus_intensity.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn cycle_repeat(&mut self) -> RepeatMode {
        self.repeat = match self.repeat {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        };
        self.repeat
    }

    pub(crate) fn repeat_mode(&self) -> RepeatMode {
        self.repeat
    }
}

/// Wall-clock playable length of a source-time `duration` at playback `speed`.
/// Speed couples pitch+tempo (the "slowed" sound), so a 0.85x track plays
/// *longer*: wall = duration / speed. Speed is floored at 0.01 to avoid a
/// divide-by-zero / runaway total. Pure — unit-tested (speed-scaling safety pin).
fn playable_wall_total(duration: Duration, speed: f32) -> Duration {
    duration.div_f32(speed.max(0.01))
}

/// Playback progress as a 0..=100 percent of `wall_total` at `position`. A zero
/// total (unknown duration) reads as 0%. Pure — unit-tested.
fn progress_percent(position: Duration, wall_total: Duration) -> f32 {
    if wall_total.is_zero() {
        0.0
    } else {
        ((position.as_secs_f32() / wall_total.as_secs_f32()) * 100.0).clamp(0.0, 100.0)
    }
}

#[derive(Default)]
pub(crate) struct ProgressSnapshot {
    pub(crate) percent: f32,
    pub(crate) seekable: bool,
    pub(crate) label: String,
}

// A pending YouTube seek: everything needed to restart the stream at a new
// position, computed on the UI thread so the actual yt-dlp/ffmpeg resolve can
// run on a background thread without holding the controller lock.
pub(crate) struct StreamSeekPlan {
    pub(crate) stream_target: String,
    pub(crate) source_target: Duration,
    pub(crate) target_wall: Duration,
    pub(crate) wall_total: Duration,
    pub(crate) percent: f32,
    pub(crate) generation: u64,
}

pub(crate) enum PlaybackAction {
    None,
    LoopStream(QueuedTrack),
    LoopLocal(Vec<PathBuf>),
    Advance,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum RepeatMode {
    Off,
    All,
    One,
}

impl RepeatMode {
    pub(crate) fn as_int(self) -> i32 {
        match self {
            RepeatMode::Off => 0,
            RepeatMode::All => 1,
            RepeatMode::One => 2,
        }
    }
}

struct FocusMuffle<S> {
    inner: S,
    enabled: Arc<AtomicBool>,
    intensity: Arc<AtomicU32>,
    last_samples: Vec<f32>,
    channel_index: usize,
    sample_rate: f32,
}

impl<S> FocusMuffle<S>
where
    S: Source,
{
    pub(crate) fn new(inner: S, enabled: Arc<AtomicBool>, intensity: Arc<AtomicU32>) -> Self {
        let sample_rate = inner.sample_rate().get() as f32;
        let channels = inner.channels().get() as usize;

        Self {
            inner,
            enabled,
            intensity,
            last_samples: vec![0.0; channels],
            channel_index: 0,
            sample_rate,
        }
    }
}

impl<S> Iterator for FocusMuffle<S>
where
    S: Source,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        let channel = self.channel_index % self.last_samples.len();
        self.channel_index = (self.channel_index + 1) % self.last_samples.len();

        if !self.enabled.load(Ordering::Relaxed) {
            self.last_samples[channel] = sample;
            return Some(sample);
        }

        let intensity = f32::from_bits(self.intensity.load(Ordering::Relaxed)).clamp(0.0, 1.0);
        let cutoff_hz = 2_200.0 + (420.0 - 2_200.0) * intensity;
        let dt = 1.0 / self.sample_rate;
        let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz);
        let alpha = dt / (rc + dt);
        let wet = 0.30 + 0.70 * intensity;
        let output_gain = 0.96 - 0.18 * intensity;

        let filtered = self.last_samples[channel] + alpha * (sample - self.last_samples[channel]);
        self.last_samples[channel] = filtered;
        Some((sample * (1.0 - wet) + filtered * wet) * output_gain)
    }
}

impl<S> Source for FocusMuffle<S>
where
    S: Source,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        let result = self.inner.try_seek(pos);
        if result.is_ok() {
            self.last_samples.fill(0.0);
        }
        result
    }
}

// --- VarSpeed --------------------------------------------------------------
// Live speed (couples pitch+tempo = the "slowed" sound) via linear-interpolation
// resampling. Crucially it OUTPUTS at the inner source's sample rate (constant),
// so rodio's mixer resampler stays valid â€” that's why a mid-track change takes
// effect immediately, unlike Player::set_speed (see [[rodio-speed-semantics]]).
// The factor is read live from an atomic each frame. factor < 1 = slowed.

struct VarSpeed<S> {
    inner: S,
    factor: Arc<AtomicU32>,
    channels: usize,
    sample_rate: rodio::SampleRate,
    prev: Vec<f32>,
    next: Vec<f32>,
    frac: f32,
    out_channel: usize,
    frame: Vec<f32>,
    started: bool,
    ended: bool,
}

impl<S> VarSpeed<S>
where
    S: Source,
{
    pub(crate) fn new(inner: S, factor: Arc<AtomicU32>) -> Self {
        let channels = inner.channels().get().max(1) as usize;
        let sample_rate = inner.sample_rate();
        Self {
            inner,
            factor,
            channels,
            sample_rate,
            prev: vec![0.0; channels],
            next: vec![0.0; channels],
            frac: 0.0,
            out_channel: 0,
            frame: vec![0.0; channels],
            started: false,
            ended: false,
        }
    }

    /// Pull one interleaved frame (all channels) from the inner source.
    fn read_frame(&mut self) -> Option<Vec<f32>> {
        let mut frame = Vec::with_capacity(self.channels);
        for _ in 0..self.channels {
            frame.push(self.inner.next()?);
        }
        Some(frame)
    }

    fn current_factor(&self) -> f32 {
        f32::from_bits(self.factor.load(Ordering::Relaxed)).clamp(0.5, 1.5)
    }
}

impl<S> Iterator for VarSpeed<S>
where
    S: Source,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.ended {
            return None;
        }
        if !self.started {
            self.prev = self.read_frame()?;
            self.next = self.read_frame().unwrap_or_else(|| self.prev.clone());
            self.started = true;
            self.out_channel = 0;
        }

        // Compute the interpolated frame at the start of each output frame.
        if self.out_channel == 0 {
            for ch in 0..self.channels {
                self.frame[ch] = self.prev[ch] + self.frac * (self.next[ch] - self.prev[ch]);
            }
        }

        let sample = self.frame[self.out_channel];
        self.out_channel += 1;

        if self.out_channel >= self.channels {
            self.out_channel = 0;
            self.frac += self.current_factor();
            while self.frac >= 1.0 {
                std::mem::swap(&mut self.prev, &mut self.next);
                match self.read_frame() {
                    Some(frame) => self.next = frame,
                    None => {
                        self.ended = true;
                        break;
                    }
                }
                self.frac -= 1.0;
            }
        }

        Some(sample)
    }
}

impl<S> Source for VarSpeed<S>
where
    S: Source,
{
    fn current_span_len(&self) -> Option<usize> {
        // Variable rate: report no span so the mixer treats output as one stream
        // at our constant sample rate (no per-span resampler rebootstrap).
        None
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        // `pos` is output (wall-clock) time; convert to input time by the factor.
        let input_pos = pos.mul_f32(self.current_factor());
        let result = self.inner.try_seek(input_pos);
        if result.is_ok() {
            self.started = false;
            self.ended = false;
            self.frac = 0.0;
            self.out_channel = 0;
        }
        result
    }
}

// --- Reverb ----------------------------------------------------------------
// A compact Schroeder/Freeverb-style reverb (4 parallel comb filters into 2
// series allpass filters, per channel). The wet/dry mix is read live from an
// atomic so the TUNE dial can adjust it without rebuilding the source. Same
// wrapper pattern as FocusMuffle, and it forwards try_seek (clearing tails).

struct CombFilter {
    buffer: Vec<f32>,
    index: usize,
    feedback: f32,
    store: f32,
    damp1: f32,
    damp2: f32,
}

impl CombFilter {
    pub(crate) fn new(len: usize, feedback: f32, damp: f32) -> Self {
        Self {
            buffer: vec![0.0; len.max(1)],
            index: 0,
            feedback,
            store: 0.0,
            damp1: damp,
            damp2: 1.0 - damp,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.buffer[self.index];
        self.store = output * self.damp2 + self.store * self.damp1;
        self.buffer[self.index] = input + self.store * self.feedback;
        self.index = (self.index + 1) % self.buffer.len();
        output
    }

    fn clear(&mut self) {
        self.buffer.iter_mut().for_each(|s| *s = 0.0);
        self.store = 0.0;
    }
}

struct AllPass {
    buffer: Vec<f32>,
    index: usize,
    feedback: f32,
}

impl AllPass {
    pub(crate) fn new(len: usize, feedback: f32) -> Self {
        Self {
            buffer: vec![0.0; len.max(1)],
            index: 0,
            feedback,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let buffered = self.buffer[self.index];
        let output = -input + buffered;
        self.buffer[self.index] = input + buffered * self.feedback;
        self.index = (self.index + 1) % self.buffer.len();
        output
    }

    fn clear(&mut self) {
        self.buffer.iter_mut().for_each(|s| *s = 0.0);
    }
}

struct ChannelReverb {
    combs: Vec<CombFilter>,
    allpasses: Vec<AllPass>,
}

impl ChannelReverb {
    pub(crate) fn new(sample_rate: u32) -> Self {
        // Freeverb tunings are defined at 44.1kHz; scale to the actual rate.
        let scale = sample_rate as f32 / 44_100.0;
        let comb_lens = [1116, 1188, 1277, 1356];
        let allpass_lens = [556, 441];
        let feedback = 0.84;
        let damp = 0.2;
        Self {
            combs: comb_lens
                .iter()
                .map(|len| CombFilter::new((*len as f32 * scale) as usize, feedback, damp))
                .collect(),
            allpasses: allpass_lens
                .iter()
                .map(|len| AllPass::new((*len as f32 * scale) as usize, 0.5))
                .collect(),
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        // Comb filters in parallel, then allpasses in series.
        let mut out: f32 = self.combs.iter_mut().map(|c| c.process(input)).sum();
        out /= self.combs.len() as f32;
        for allpass in &mut self.allpasses {
            out = allpass.process(out);
        }
        out
    }

    fn clear(&mut self) {
        self.combs.iter_mut().for_each(CombFilter::clear);
        self.allpasses.iter_mut().for_each(AllPass::clear);
    }
}

struct Reverb<S> {
    inner: S,
    mix: Arc<AtomicU32>,
    channels: Vec<ChannelReverb>,
    channel_index: usize,
}

impl<S> Reverb<S>
where
    S: Source,
{
    pub(crate) fn new(inner: S, mix: Arc<AtomicU32>) -> Self {
        let sample_rate = inner.sample_rate().get();
        let channel_count = inner.channels().get().max(1) as usize;
        let channels = (0..channel_count)
            .map(|_| ChannelReverb::new(sample_rate))
            .collect();
        Self {
            inner,
            mix,
            channels,
            channel_index: 0,
        }
    }
}

impl<S> Iterator for Reverb<S>
where
    S: Source,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        let channel = self.channel_index % self.channels.len();
        self.channel_index = (self.channel_index + 1) % self.channels.len();

        let mix = f32::from_bits(self.mix.load(Ordering::Relaxed));
        if mix <= 0.0 {
            return Some(sample);
        }
        let wet = self.channels[channel].process(sample);
        // Keep the dry signal mostly intact and layer wet on top so the mix
        // slider deepens the space without washing the track out.
        Some(sample * (1.0 - 0.3 * mix) + wet * mix)
    }
}

impl<S> Source for Reverb<S>
where
    S: Source,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        let result = self.inner.try_seek(pos);
        if result.is_ok() {
            self.channels.iter_mut().for_each(ChannelReverb::clear);
        }
        result
    }
}

pub(crate) fn first_track_duration(tracks: &[PathBuf]) -> AppResult<Option<Duration>> {
    let Some(first_track) = tracks.first() else {
        return Ok(None);
    };

    if tracks.len() != 1 {
        return Ok(None);
    }

    // The decoder reports duration instantly for WAV/FLAC/OGG, but returns None
    // for most MP3s (no frame scan). Fall back to probing with bundled ffmpeg so
    // local progress + seek work for downloads (which are .mp3).
    let file = File::open(first_track)?;
    if let Some(duration) = rodio::Decoder::try_from(file)?.total_duration() {
        if !duration.is_zero() {
            return Ok(Some(duration));
        }
    }
    Ok(probe_audio_duration(first_track))
}

// Probe a media file's duration via the bundled ffmpeg. `ffmpeg -i <file>` with
// no output prints "Duration: HH:MM:SS.ss" to stderr and exits non-zero â€” that
// is expected; we only parse the banner.
fn probe_audio_duration(path: &Path) -> Option<Duration> {
    // Do not spawn ffmpeg while a streaming-engine repair is running.
    if streaming_helpers::repair_in_progress() {
        return None;
    }
    let mut command = hidden_command("ffmpeg");
    command
        // Never let ffmpeg block waiting on stdin (e.g. an overwrite prompt) â€”
        // that would wedge the background playback thread indefinitely.
        .stdin(Stdio::null())
        .arg("-nostdin")
        .arg("-hide_banner")
        .arg("-i")
        .arg(path);
    let output = command_output_with_timeout(command, Duration::from_secs(15)).ok()?;
    parse_ffmpeg_duration(&String::from_utf8_lossy(&output.stderr))
}

fn parse_ffmpeg_duration(text: &str) -> Option<Duration> {
    let start = text.find("Duration:")? + "Duration:".len();
    let token = text[start..].split(',').next()?.trim();
    if token.starts_with("N/A") {
        return None;
    }
    let mut parts = token.split(':');
    let hours: f64 = parts.next()?.trim().parse().ok()?;
    let minutes: f64 = parts.next()?.trim().parse().ok()?;
    let seconds: f64 = parts.next()?.trim().parse().ok()?;
    let total = hours * 3600.0 + minutes * 60.0 + seconds;
    (total > 0.0).then(|| Duration::from_secs_f64(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Playback engine: speed-scaling safety pin --------------------------
    // Pins the source-time -> wall-clock math that `progress_snapshot`,
    // `seek_local`, and `prepare_stream_seek` share, so the player.rs move (and
    // any future tune work) can't silently change seek/progress behavior.
    #[test]
    fn progress_snapshot_speed_scaling_is_pinned() {
        let d = Duration::from_secs(100);
        // 1.0x: wall-clock total equals source time.
        assert_eq!(playable_wall_total(d, 1.0), Duration::from_secs(100));
        // 0.5x (slowed) plays twice as long; 2.0x half as long.
        assert_eq!(playable_wall_total(d, 0.5), Duration::from_secs(200));
        assert_eq!(playable_wall_total(d, 2.0), Duration::from_secs(50));
        // Speed is floored at 0.01 (no divide-by-zero / runaway total).
        assert_eq!(playable_wall_total(d, 0.0), playable_wall_total(d, 0.01));

        // Percent is measured against the wall total, not the source duration.
        let pos = Duration::from_secs(50);
        assert!((progress_percent(pos, playable_wall_total(d, 1.0)) - 50.0).abs() < 0.01);
        // Same 50s position is only 25% of the 200s wall length at 0.5x.
        assert!((progress_percent(pos, playable_wall_total(d, 0.5)) - 25.0).abs() < 0.01);
        // Zero total reads 0%, and progress is clamped to 100%.
        assert_eq!(progress_percent(pos, Duration::ZERO), 0.0);
        assert_eq!(progress_percent(Duration::from_secs(999), d), 100.0);
    }
}
