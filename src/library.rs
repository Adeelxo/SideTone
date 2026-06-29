//! Local library: filesystem scan (depth-bounded, symlink-skipping), the
//! library.json index with drive-aware prune protection, and slot conversion.
//! Extracted verbatim from `main.rs` (move-only; no logic changes).

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::domain::{Collection, Track, YtResultSlot};
use crate::persistence::{data_dir, write_json_atomic};
use crate::{timestamp_millis, AppResult};

const SUPPORTED_EXTENSIONS: &[&str] = &["aac", "aiff", "flac", "m4a", "mp3", "ogg", "opus", "wav"];
const MAX_SCAN_DEPTH: usize = 32;

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct LibraryIndex {
    version: u32,
    root: Option<String>,
    scanned_at_ms: u64,
    pub(crate) tracks: Vec<YtResultSlot>,
}

fn library_index_path() -> Option<PathBuf> {
    Some(data_dir()?.join("library.json"))
}

fn load_library_index() -> LibraryIndex {
    library_index_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<LibraryIndex>(&text).ok())
        .unwrap_or_default()
}

fn prune_missing_library_tracks(mut index: LibraryIndex) -> (LibraryIndex, usize) {
    let before = index.tracks.len();
    index.tracks.retain(|track| Path::new(&track.url).is_file());
    let removed = before - index.tracks.len();
    (index, removed)
}

/// Status shown when we decline to prune because the library's storage looks
/// unavailable (so a transient disconnect can't wipe the curated index).
const LIBRARY_UNAVAILABLE_MSG: &str = "Library drive unavailable — keeping saved tracks.";

/// True if the library's saved root is currently reachable. A `None`/empty root
/// (legacy indexes written before the root was recorded) is treated as available
/// so we still clean those normally.
fn library_root_available(index: &LibraryIndex) -> bool {
    match index.root.as_deref() {
        Some(root) if !root.is_empty() => Path::new(root).is_dir(),
        _ => true,
    }
}

/// A prune that removes every row — or nearly every row of a non-trivial index —
/// almost always means the storage went away (unplugged external/network drive),
/// not that the user deleted that many files at once. In that case we must not
/// touch the index. Small libraries are exempt so deleting 2 of 3 files still
/// prunes normally. Pure + unit-tested.
fn prune_is_total_or_near_total(before: usize, removed: usize) -> bool {
    if before == 0 || removed == 0 {
        return false;
    }
    if removed == before {
        return true; // total wipe
    }
    // ">=90% of a library with at least 10 entries" — a near-total disappearance.
    before >= 10 && removed * 10 >= before * 9
}

pub(crate) fn load_clean_library_index() -> (LibraryIndex, usize, Option<String>) {
    let index = load_library_index();

    // If the library's root drive is unplugged/unreachable, every file will look
    // "missing". Skip pruning entirely so a transient disconnect can't erase the
    // user's curated index. The audio files themselves are never deleted here —
    // this only protects the saved index.
    if !library_root_available(&index) {
        return (index, 0, Some(LIBRARY_UNAVAILABLE_MSG.to_string()));
    }

    let before = index.tracks.len();
    let original = index.clone();
    let (cleaned, removed) = prune_missing_library_tracks(index);

    // Defense in depth: even with a present root, refuse to persist a prune that
    // would erase all/nearly all rows — treat that as storage trouble and keep
    // the original index intact.
    if prune_is_total_or_near_total(before, removed) {
        return (original, 0, Some(LIBRARY_UNAVAILABLE_MSG.to_string()));
    }

    let save_error = if removed > 0 {
        save_library_index(&cleaned)
            .err()
            .map(|error| format!("Could not update library.json: {error}"))
    } else {
        None
    };
    (cleaned, removed, save_error)
}

fn save_library_index(index: &LibraryIndex) -> AppResult<()> {
    let Some(path) = library_index_path() else {
        return Ok(());
    };
    write_json_atomic(&path, index)
}

pub(crate) fn save_library_scan(root: &Path, tracks: &[YtResultSlot]) -> AppResult<()> {
    save_library_index(&LibraryIndex {
        version: 1,
        root: Some(root.to_string_lossy().to_string()),
        scanned_at_ms: timestamp_millis().min(u64::MAX as u128) as u64,
        tracks: tracks.to_vec(),
    })
}

pub(crate) fn save_library_tracks(tracks: &[YtResultSlot]) -> AppResult<()> {
    let mut index = load_library_index();
    index.version = 1;
    index.scanned_at_ms = timestamp_millis().min(u64::MAX as u128) as u64;
    index.tracks = tracks.to_vec();
    save_library_index(&index)
}

pub(crate) fn local_slots_for_collections(collections: &[Collection]) -> Vec<YtResultSlot> {
    collections
        .iter()
        .flat_map(|collection| {
            collection.tracks.iter().map(move |track| YtResultSlot {
                title: format!("{}  {}", collection.name, display_track_name(&track.path)),
                url: track.path.to_string_lossy().to_string(),
            })
        })
        .collect()
}

pub(crate) fn scan_library(root: &Path) -> io::Result<Vec<Collection>> {
    let mut grouped: BTreeMap<String, Vec<Track>> = BTreeMap::new();
    scan_dir(root, root, &mut grouped, 0)?;

    Ok(grouped
        .into_iter()
        .map(|(name, mut tracks)| {
            tracks.sort_by(|left, right| {
                left.path
                    .to_string_lossy()
                    .to_lowercase()
                    .cmp(&right.path.to_string_lossy().to_lowercase())
            });
            Collection { name, tracks }
        })
        .collect())
}

pub(crate) fn tracks_for_folder(root: &Path) -> io::Result<Vec<PathBuf>> {
    let collections = scan_library(root)?;
    let mut tracks = Vec::new();

    for collection in collections {
        for track in collection.tracks {
            tracks.push(track.path);
        }
    }

    Ok(tracks)
}

fn scan_dir(
    root: &Path,
    current: &Path,
    grouped: &mut BTreeMap<String, Vec<Track>>,
    depth: usize,
) -> io::Result<()> {
    if depth > MAX_SCAN_DEPTH {
        return Ok(());
    }

    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("Skipping unreadable folder {}: {error}", current.display());
            return Ok(());
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!("Skipping unreadable folder entry: {error}");
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                eprintln!("Skipping unknown file type {}: {error}", path.display());
                continue;
            }
        };

        if file_type.is_symlink() {
            continue;
        }

        if file_type.is_dir() {
            scan_dir(root, &path, grouped, depth + 1)?;
            continue;
        }

        if !file_type.is_file() || !is_supported_audio_file(&path) {
            continue;
        }

        let collection_name = collection_name_for(root, &path);
        grouped
            .entry(collection_name)
            .or_default()
            .push(Track { path });
    }

    Ok(())
}

pub(crate) fn is_supported_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|extension| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
        .unwrap_or(false)
}

fn collection_name_for(root: &Path, track_path: &Path) -> String {
    let Some(parent) = track_path.parent() else {
        return "Unsorted".to_string();
    };

    if parent == root {
        return "Unsorted".to_string();
    }

    parent
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("Unknown")
        .to_string()
}

pub(crate) fn display_track_name(path: &Path) -> String {
    match path.file_name().and_then(OsStr::to_str) {
        Some(file_name) => file_name.to_string(),
        None => path.to_string_lossy().to_string(),
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
    fn library_prune_removes_missing_index_rows_only() {
        let dir = unique_test_dir("library-prune");
        let existing = dir.join("keep.mp3");
        fs::write(&existing, b"not-real-audio").expect("write placeholder");
        let missing = dir.join("missing.mp3");
        let index = LibraryIndex {
            version: 1,
            root: Some(dir.to_string_lossy().to_string()),
            scanned_at_ms: 1,
            tracks: vec![
                YtResultSlot {
                    title: "Keep".to_string(),
                    url: existing.to_string_lossy().to_string(),
                },
                YtResultSlot {
                    title: "Missing".to_string(),
                    url: missing.to_string_lossy().to_string(),
                },
            ],
        };

        let (cleaned, removed) = prune_missing_library_tracks(index);

        assert_eq!(removed, 1);
        assert_eq!(cleaned.tracks.len(), 1);
        assert_eq!(cleaned.tracks[0].title, "Keep");
        assert!(existing.exists(), "real user file must not be deleted");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn library_root_unavailable_when_root_dir_is_missing() {
        let index = LibraryIndex {
            version: 1,
            root: Some("Z:/definitely/not/mounted/sidetone-library".to_string()),
            scanned_at_ms: 1,
            tracks: vec![YtResultSlot {
                title: "Song".to_string(),
                url: "Z:/definitely/not/mounted/sidetone-library/song.mp3".to_string(),
            }],
        };
        assert!(
            !library_root_available(&index),
            "an unmounted root must report unavailable"
        );

        // A legacy index with no recorded root is treated as available so it
        // still gets cleaned normally.
        let legacy = LibraryIndex {
            root: None,
            ..index.clone()
        };
        assert!(library_root_available(&legacy));
    }

    #[test]
    fn library_root_available_when_root_dir_exists() {
        let dir = unique_test_dir("library-root-available");
        let index = LibraryIndex {
            version: 1,
            root: Some(dir.to_string_lossy().to_string()),
            scanned_at_ms: 1,
            tracks: Vec::new(),
        };
        assert!(library_root_available(&index));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_total_or_near_total_is_treated_as_storage_loss() {
        // A complete disappearance is always suspicious.
        assert!(prune_is_total_or_near_total(50, 50));
        // >=90% of a non-trivial index.
        assert!(prune_is_total_or_near_total(10, 9));
        assert!(prune_is_total_or_near_total(100, 95));

        // Normal cleanup of a few files is NOT treated as storage loss.
        assert!(!prune_is_total_or_near_total(100, 1));
        assert!(!prune_is_total_or_near_total(100, 50));
        // Small libraries are exempt so deleting 2 of 3 still prunes.
        assert!(!prune_is_total_or_near_total(3, 2));
        // ...but a total wipe is suspicious even for a tiny library.
        assert!(prune_is_total_or_near_total(3, 3));
        // Degenerate inputs.
        assert!(!prune_is_total_or_near_total(0, 0));
        assert!(!prune_is_total_or_near_total(5, 0));
    }

    #[test]
    fn local_slots_include_full_scan_results() {
        let tracks: Vec<Track> = (0..305)
            .map(|index| Track {
                path: PathBuf::from(format!("C:/Music/song-{index}.mp3")),
            })
            .collect();
        let collections = vec![Collection {
            name: "Folder".to_string(),
            tracks,
        }];

        let slots = local_slots_for_collections(&collections);

        assert_eq!(slots.len(), 305);
        assert!(slots.iter().any(|slot| slot.url.ends_with("song-304.mp3")));
    }

    #[test]
    fn supported_audio_extensions_are_case_insensitive() {
        for extension in ["mp3", "FLAC", "M4A", "ogg", "OPUS", "wav", "aac", "AIFF"] {
            assert!(
                is_supported_audio_file(Path::new(&format!("track.{extension}"))),
                "{extension} should be accepted"
            );
        }
        assert!(!is_supported_audio_file(Path::new("cover.jpg")));
        assert!(!is_supported_audio_file(Path::new("README")));
    }

    #[test]
    fn scan_library_ignores_symlinked_audio_files() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let base =
                std::env::temp_dir().join(format!("sidetone-scan-test-{}", std::process::id()));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).expect("create scan test dir");
            let real = base.join("real.mp3");
            let linked = base.join("linked.mp3");
            fs::write(&real, b"x").expect("write real file");
            symlink(&real, &linked).expect("create symlink");

            let collections = scan_library(&base).expect("scan library");
            let tracks: Vec<PathBuf> = collections
                .into_iter()
                .flat_map(|collection| collection.tracks.into_iter().map(|track| track.path))
                .collect();
            assert!(tracks.iter().any(|path| path == &real));
            assert!(!tracks.iter().any(|path| path == &linked));

            let _ = fs::remove_dir_all(&base);
        }
    }
}
