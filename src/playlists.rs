//! Playlist storage: sanitized-name file stems, collision-safe saves, and
//! load/list/delete over the playlists data folder. Extracted verbatim from
//! `main.rs` (move-only; no logic changes).

use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::{Playlist, YtResultSlot};
use crate::persistence::{data_dir, write_json_atomic};
use crate::AppResult;

fn playlists_dir() -> AppResult<PathBuf> {
    Ok(data_dir()
        .ok_or("could not resolve app data folder for playlists")?
        .join("playlists"))
}

// Turn a playlist name into a safe file stem.
fn sanitize_playlist_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Cap length so the resulting filename stays well under filesystem limits.
    let cleaned: String = cleaned.trim().chars().take(64).collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        return "playlist".to_string();
    }
    // Avoid Windows reserved device names (CON, PRN, NUL, COM1, LPT1, â€¦) which
    // can't be used as filenames even with an extension.
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if reserved.iter().any(|r| r.eq_ignore_ascii_case(&cleaned)) {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

pub(crate) fn save_playlist(name: &str, tracks: &[YtResultSlot]) -> AppResult<()> {
    let dir = playlists_dir()?;
    save_playlist_to_dir(&dir, name, tracks)
}

fn save_playlist_to_dir(dir: &Path, name: &str, tracks: &[YtResultSlot]) -> AppResult<()> {
    fs::create_dir_all(dir)?;
    let stem = sanitize_playlist_name(name);
    let path = dir.join(format!("{stem}.json"));
    let display_name = name.trim().to_string();
    if path.exists() {
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(existing) = serde_json::from_str::<Playlist>(&text) {
                if existing.name != display_name {
                    return Err(format!(
                        "playlist name conflicts with existing '{}'. Pick a different name.",
                        existing.name
                    )
                    .into());
                }
            }
        }
    }
    let playlist = Playlist {
        name: display_name,
        tracks: tracks.to_vec(),
    };
    write_json_atomic(&path, &playlist)?;
    Ok(())
}

pub(crate) fn list_playlists() -> AppResult<Vec<Playlist>> {
    let dir = playlists_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut playlists: Vec<Playlist> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|path| fs::read_to_string(&path).ok())
        .filter_map(|text| serde_json::from_str::<Playlist>(&text).ok())
        .collect();
    playlists.sort_by_key(|playlist| playlist.name.to_lowercase());
    Ok(playlists)
}

pub(crate) fn load_playlist(name: &str) -> AppResult<Playlist> {
    let dir = playlists_dir()?;
    let stem = sanitize_playlist_name(name);
    let path = dir.join(format!("{stem}.json"));
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str::<Playlist>(&text)?)
}

pub(crate) fn delete_playlist(name: &str) -> AppResult<()> {
    let dir = playlists_dir()?;
    let stem = sanitize_playlist_name(name);
    let path = dir.join(format!("{stem}.json"));
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
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
    fn playlist_name_sanitization_blocks_path_tricks() {
        for evil in [
            "../../etc/passwd",
            "..\\..\\windows\\system32",
            "a/b/c",
            "name:with*bad?chars",
            "....//....//",
        ] {
            let s = sanitize_playlist_name(evil);
            assert!(!s.is_empty());
            assert!(!s.contains('/'), "slash survived: {s}");
            assert!(!s.contains('\\'), "backslash survived: {s}");
            assert!(!s.contains(".."), "traversal survived: {s}");
            assert!(!s.contains(':'), "colon survived: {s}");
        }
        // Empty / whitespace fall back to a safe default.
        assert_eq!(sanitize_playlist_name(""), "playlist");
        assert_eq!(sanitize_playlist_name("   "), "playlist");
        // Windows reserved device names are escaped.
        assert!(sanitize_playlist_name("CON").starts_with('_'));
        assert!(sanitize_playlist_name("nul").starts_with('_'));
        // Length is capped.
        assert!(sanitize_playlist_name(&"x".repeat(300)).chars().count() <= 64);
        // Ordinary names are preserved.
        assert_eq!(sanitize_playlist_name("My Mix 2024"), "My Mix 2024");
    }

    #[test]
    fn playlist_save_refuses_sanitized_name_collision() {
        let dir = unique_test_dir("playlist-collision");
        let tracks = [YtResultSlot {
            title: "One".to_string(),
            url: "https://example.test/one".to_string(),
        }];

        save_playlist_to_dir(&dir, "A/B", &tracks).expect("first playlist saves");
        let error = save_playlist_to_dir(&dir, "A?B", &tracks)
            .expect_err("different names that sanitize to the same file must fail");
        assert!(error.to_string().contains("conflicts"));

        let saved = fs::read_to_string(dir.join("A_B.json")).expect("existing playlist remains");
        let playlist: Playlist = serde_json::from_str(&saved).expect("valid playlist json");
        assert_eq!(playlist.name, "A/B");
        let _ = fs::remove_dir_all(&dir);
    }
}
