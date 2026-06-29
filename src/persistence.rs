//! Persistence layer: data-directory resolution, atomic JSON/text writes, and
//! the theme/layout config files. Extracted verbatim from `main.rs` (move-only;
//! no logic changes). Items used outside this module are `pub(crate)`.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{timestamp_millis, AppResult};

/// The directory the running executable lives in (where helpers + `portable.flag`
/// sit). Helper binaries are always resolved from here, never the data dir.
pub(crate) fn exe_dir() -> Option<PathBuf> {
    Some(env::current_exe().ok()?.parent()?.to_path_buf())
}

/// Portable mode = a `portable.flag` file next to `sidetone.exe`. When present,
/// SideTone keeps all user data exe-adjacent (the pre-v7 behavior) instead of
/// using `%LOCALAPPDATA%`. Opt-in only.
pub(crate) fn portable_mode() -> bool {
    exe_dir()
        .map(|dir| dir.join("portable.flag").exists())
        .unwrap_or(false)
}

/// Pure resolver for the Windows data directory, factored out so it can be
/// unit-tested without depending on the real executable location. Decision:
/// - `portable.flag` next to the exe â†’ keep data exe-adjacent (`exe_dir`).
/// - otherwise â†’ `<LOCALAPPDATA>\SideTone\Data`.
/// - if `%LOCALAPPDATA%` is unavailable, fall back to exe-adjacent so the app
///   still has somewhere to write (never panics).
#[cfg_attr(not(windows), allow(dead_code))]
fn resolve_windows_data_dir(exe_dir: &Path, local_appdata: Option<&Path>) -> PathBuf {
    if exe_dir.join("portable.flag").exists() {
        return exe_dir.to_path_buf();
    }
    match local_appdata {
        Some(base) => base.join("SideTone").join("Data"),
        None => exe_dir.to_path_buf(),
    }
}

/// Where SideTone keeps its config + library (theme/layout/hotkeys/tune JSON,
/// the downloads and playlists folders). Centralized so the location is
/// platform-correct in one place:
/// - **Windows:** `%LOCALAPPDATA%\SideTone\Data` (v7) â€” or next to the .exe if a
///   `portable.flag` is present (opt-in portable mode).
/// - **macOS:** ~/Library/Application Support/SideTone (the .app bundle is
///   read-only, so we can't write next to the binary).
/// - **other (Linux):** ~/.local/share/sidetone.
pub(crate) fn data_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let dir = {
        let exe = exe_dir()?;
        let local = env::var_os("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        let dir = resolve_windows_data_dir(&exe, local.as_deref());
        // Best-effort create; callers that write also create as needed.
        let _ = fs::create_dir_all(&dir);
        dir
    };
    #[cfg(not(windows))]
    let dir = {
        let sub = if cfg!(target_os = "macos") {
            "Library/Application Support/SideTone"
        } else {
            ".local/share/sidetone"
        };
        let dir = PathBuf::from(env::var("HOME").ok()?).join(sub);
        fs::create_dir_all(&dir).ok()?;
        dir
    };
    Some(dir)
}

fn atomic_write_text(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("sidetone-data");
    let tmp = path.with_file_name(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        timestamp_millis()
    ));
    fs::write(&tmp, text)?;
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> AppResult<()> {
    let json = serde_json::to_string_pretty(value)?;
    atomic_write_text(path, &json)?;
    Ok(())
}

#[derive(Default, Serialize, Deserialize)]
struct ThemeConfig {
    theme: i32,
}

fn theme_config_path() -> Option<PathBuf> {
    Some(data_dir()?.join("theme.json"))
}

pub(crate) fn load_theme_config() -> i32 {
    let theme = theme_config_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<ThemeConfig>(&text).ok())
        .map(|config| config.theme)
        .unwrap_or(0);

    theme.clamp(0, 3)
}

pub(crate) fn save_theme_config(theme: i32) {
    if let Some(path) = theme_config_path() {
        let config = ThemeConfig {
            theme: theme.clamp(0, 3),
        };
        let _ = write_json_atomic(&path, &config);
    }
}

#[derive(Default, Serialize, Deserialize)]
struct LayoutConfig {
    mode: i32,
}

fn layout_config_path() -> Option<PathBuf> {
    Some(data_dir()?.join("layout.json"))
}

pub(crate) fn load_layout_config() -> i32 {
    layout_config_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<LayoutConfig>(&text).ok())
        .map(|config| config.mode)
        .unwrap_or(0)
        .clamp(0, 1)
}

pub(crate) fn save_layout_config(mode: i32) {
    if let Some(path) = layout_config_path() {
        let config = LayoutConfig {
            mode: mode.clamp(0, 1),
        };
        let _ = write_json_atomic(&path, &config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn unique_test_dir(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("sidetone-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    #[test]
    fn default_windows_data_path_is_localappdata_sidetone_data() {
        let exe = unique_test_dir("dataresolve-exe");
        let local = unique_test_dir("dataresolve-local");
        // No portable.flag â†’ use %LOCALAPPDATA%\SideTone\Data.
        let resolved = resolve_windows_data_dir(&exe, Some(&local));
        assert_eq!(resolved, local.join("SideTone").join("Data"));
        let _ = fs::remove_dir_all(&exe);
        let _ = fs::remove_dir_all(&local);
    }

    #[test]
    fn portable_flag_selects_exe_adjacent_data() {
        let exe = unique_test_dir("portable-exe");
        let local = unique_test_dir("portable-local");
        fs::write(exe.join("portable.flag"), b"").expect("write flag");
        // portable.flag present â†’ data stays next to the exe, ignoring LOCALAPPDATA.
        let resolved = resolve_windows_data_dir(&exe, Some(&local));
        assert_eq!(resolved, exe);
        let _ = fs::remove_dir_all(&exe);
        let _ = fs::remove_dir_all(&local);
    }

    #[test]
    fn missing_localappdata_falls_back_to_exe_dir() {
        let exe = unique_test_dir("fallback-exe");
        // No portable flag and no LOCALAPPDATA â†’ fall back to exe dir (no panic).
        let resolved = resolve_windows_data_dir(&exe, None);
        assert_eq!(resolved, exe);
        let _ = fs::remove_dir_all(&exe);
    }
}
