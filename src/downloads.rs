//! Managed downloads folder: the downloads dir, the canonicalized delete-boundary
//! guard, and listing/sorting downloaded audio into slots. Extracted verbatim
//! from `main.rs` (move-only; no logic changes).

use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::YtResultSlot;
use crate::library::{display_track_name, is_supported_audio_file};
use crate::persistence::data_dir;
use crate::AppResult;

pub(crate) fn favorites_dir() -> AppResult<PathBuf> {
    Ok(data_dir()
        .ok_or("could not resolve app data folder for downloads")?
        .join("downloads"))
}

/// Safety guard for the "Delete download" actions: only ever remove files that
/// actually live inside our managed downloads folder. Both paths are canonicalized
/// (resolving symlinks/`..`), so a stray or crafted path elsewhere â€” a scanned
/// library file, a playlist entry pointing outside â€” can never be deleted by the
/// app. Returns false if either path can't be resolved (e.g. file already gone).
pub(crate) fn path_within_downloads(path: &Path) -> bool {
    match favorites_dir() {
        Ok(dir) => path_contained_in(&dir, path),
        Err(_) => false,
    }
}

/// True only if `path` resolves to a location inside `dir`. Both are canonicalized
/// first (resolving `..` and symlinks), so traversal can't escape and a path that
/// doesn't exist (can't be canonicalized) is denied. Pure + unit-tested.
fn path_contained_in(dir: &Path, path: &Path) -> bool {
    match (dir.canonicalize(), path.canonicalize()) {
        (Ok(dir), Ok(path)) => path.starts_with(dir),
        _ => false,
    }
}

pub(crate) fn favorite_audio_files() -> AppResult<Vec<PathBuf>> {
    let favorites = favorites_dir()?;
    if !favorites.exists() {
        return Ok(Vec::new());
    }

    Ok(fs::read_dir(favorites)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_supported_audio_file(path))
        .collect())
}

pub(crate) fn newest_favorite_file() -> AppResult<Option<PathBuf>> {
    let mut files = favorite_audio_files()?;
    files.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    Ok(files.pop())
}

pub(crate) fn favorite_slots() -> AppResult<Vec<YtResultSlot>> {
    let mut files = favorite_audio_files()?;
    files.sort_by(|left, right| {
        display_track_name(left)
            .to_lowercase()
            .cmp(&display_track_name(right).to_lowercase())
    });

    Ok(files
        .into_iter()
        .map(|path| YtResultSlot {
            title: display_track_name(&path),
            url: path.to_string_lossy().to_string(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_deletion_is_confined_to_its_folder() {
        use std::fs;
        let base = std::env::temp_dir().join(format!("sidetone-test-{}", std::process::id()));
        let downloads = base.join("downloads");
        fs::create_dir_all(&downloads).expect("create temp downloads");
        let inside = downloads.join("song.mp3");
        fs::write(&inside, b"x").expect("write inside");
        let outside = base.join("outside.mp3");
        fs::write(&outside, b"x").expect("write outside");

        // A real file inside the folder is allowed.
        assert!(path_contained_in(&downloads, &inside));
        // A file outside the folder is denied.
        assert!(!path_contained_in(&downloads, &outside));
        // Traversal that escapes the folder is denied (canonicalize resolves ..).
        assert!(!path_contained_in(
            &downloads,
            &downloads.join("..").join("outside.mp3")
        ));
        // A path that doesn't exist can't be canonicalized â†’ denied.
        assert!(!path_contained_in(&downloads, &downloads.join("ghost.mp3")));

        let _ = fs::remove_dir_all(&base);
    }
}
