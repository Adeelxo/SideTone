//! v7 one-time data migration: copy pre-v7 exe-adjacent user data into the new
//! `%LOCALAPPDATA%\SideTone\Data` location. Extracted verbatim from `main.rs`
//! (move-only; no logic changes). Copy-then-verify, never destructive.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use crate::persistence::{data_dir, exe_dir, portable_mode};

/// Marker written into the new data dir after a v7 migration attempt, so we
/// never re-run migration (and never re-copy over data the user has since
/// changed). See [`migrate_legacy_data_if_needed`].
const MIGRATION_MARKER: &str = ".migration-v7-complete";

/// Names of the legacy exe-adjacent data that v7 migrates into the new data dir.
const MIGRATED_FILES: [&str; 5] = [
    "theme.json",
    "layout.json",
    "hotkeys.json",
    "tune.json",
    "library.json",
];
const MIGRATED_DIRS: [&str; 2] = ["downloads", "playlists"];

/// One-time, safe migration of pre-v7 exe-adjacent user data into the new
/// `%LOCALAPPDATA%\SideTone\Data` folder. Windows, non-portable only. Called once
/// at startup, before any data is read. Copy-then-verify, never destructive:
/// the old files are left exactly where they were. Any failure is swallowed so
/// the app still launches.
#[cfg(windows)]
pub(crate) fn migrate_legacy_data_if_needed() {
    if portable_mode() {
        return;
    }
    let Some(old_dir) = exe_dir() else {
        return;
    };
    let Some(new_dir) = data_dir() else {
        return;
    };
    // If we're somehow already running from inside the data dir, nothing to do.
    if old_dir == new_dir {
        return;
    }
    // Best-effort: a failure here must not crash startup.
    let _ = run_migration(&old_dir, &new_dir);
}

/// Core migration: skip if the marker already exists, otherwise copy the legacy
/// data and â€” **only if the copy fully succeeds** â€” write the marker. A failed
/// or partial copy leaves no marker, so the next launch retries cleanly. Pure
/// w.r.t. its directory arguments, so it is unit-tested directly.
#[cfg_attr(not(windows), allow(dead_code))]
fn run_migration(old_dir: &Path, new_dir: &Path) -> io::Result<()> {
    let marker = new_dir.join(MIGRATION_MARKER);
    if marker.exists() {
        return Ok(());
    }
    fs::create_dir_all(new_dir)?;
    copy_legacy_data(old_dir, new_dir)?;
    // Marker only on full success â†’ a failed/partial migration can retry later.
    fs::write(&marker, b"SideTone v7 data migration complete\n")?;
    Ok(())
}

/// Copy the legacy data items from `old_dir` into `new_dir`, skipping any item
/// whose target already exists (never overwrite newer data). Each item is staged
/// into a temp sibling and verified before being renamed into place, so a
/// half-copied `downloads/`/`playlists/` never lands at the final path (which
/// would make future runs skip it). Platform-agnostic, unit-tested directly.
#[cfg_attr(not(windows), allow(dead_code))]
fn copy_legacy_data(old_dir: &Path, new_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(new_dir)?;
    for name in MIGRATED_FILES {
        let src = old_dir.join(name);
        let dst = new_dir.join(name);
        if src.is_file() && !dst.exists() {
            copy_file_into_place(&src, &dst)?;
        }
    }
    for name in MIGRATED_DIRS {
        let src = old_dir.join(name);
        let dst = new_dir.join(name);
        if src.is_dir() && !dst.exists() {
            copy_dir_into_place(&src, &dst)?;
        }
    }
    Ok(())
}

/// A temp sibling path next to `dst` (same parent), used to stage a copy before
/// the atomic rename into place.
#[cfg_attr(not(windows), allow(dead_code))]
fn migrate_temp_path(dst: &Path) -> PathBuf {
    let mut name = dst.file_name().map(|n| n.to_owned()).unwrap_or_default();
    name.push(".migrate-tmp");
    dst.with_file_name(name)
}

/// Stage a file copy in a temp sibling, verify its size, then rename into place.
/// On any failure the temp file is removed and the final `dst` is left untouched.
#[cfg_attr(not(windows), allow(dead_code))]
fn copy_file_into_place(src: &Path, dst: &Path) -> io::Result<()> {
    let tmp = migrate_temp_path(dst);
    let _ = fs::remove_file(&tmp);
    if let Err(error) = copy_file_verified(src, &tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    if let Err(error) = fs::rename(&tmp, dst) {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

/// Stage a directory copy in a temp sibling, verify it, then rename into place.
/// On any failure the temp tree is removed and the final `dst` is left untouched.
#[cfg_attr(not(windows), allow(dead_code))]
fn copy_dir_into_place(src: &Path, dst: &Path) -> io::Result<()> {
    let tmp = migrate_temp_path(dst);
    let _ = fs::remove_dir_all(&tmp);
    if let Err(error) = copy_dir_verified(src, &tmp) {
        let _ = fs::remove_dir_all(&tmp);
        return Err(error);
    }
    if let Err(error) = fs::rename(&tmp, dst) {
        let _ = fs::remove_dir_all(&tmp);
        return Err(error);
    }
    Ok(())
}

/// Copy a single file and verify the destination exists with a matching length.
#[cfg_attr(not(windows), allow(dead_code))]
fn copy_file_verified(src: &Path, dst: &Path) -> io::Result<()> {
    fs::copy(src, dst)?;
    let src_len = fs::metadata(src)?.len();
    let dst_len = fs::metadata(dst)?.len();
    if src_len != dst_len {
        return Err(io::Error::other("copied file size did not match source"));
    }
    Ok(())
}

/// Recursively copy a directory tree into `dst` (expected to be a fresh temp
/// path), verifying each copied file.
#[cfg_attr(not(windows), allow(dead_code))]
fn copy_dir_verified(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_verified(&from, &to)?;
        } else if file_type.is_file() {
            copy_file_verified(&from, &to)?;
        }
        // Symlinks/other types are skipped (none are expected in our data).
    }
    if !dst.is_dir() {
        return Err(io::Error::other("copied directory was not created"));
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
    fn migration_copies_legacy_data_without_deleting_source() {
        let old = unique_test_dir("mig-old");
        let new = unique_test_dir("mig-new");
        fs::write(old.join("theme.json"), b"{\"theme\":2}").unwrap();
        fs::write(old.join("hotkeys.json"), b"{}").unwrap();
        let dl = old.join("downloads");
        fs::create_dir_all(&dl).unwrap();
        fs::write(dl.join("song.mp3"), b"audio-bytes").unwrap();
        let pl = old.join("playlists");
        fs::create_dir_all(&pl).unwrap();
        fs::write(pl.join("mix.json"), b"[]").unwrap();

        run_migration(&old, &new).expect("migration ok");

        // Copied to the new dir...
        assert_eq!(fs::read(new.join("theme.json")).unwrap(), b"{\"theme\":2}");
        assert!(new.join("hotkeys.json").is_file());
        assert_eq!(
            fs::read(new.join("downloads").join("song.mp3")).unwrap(),
            b"audio-bytes"
        );
        assert!(new.join("playlists").join("mix.json").is_file());
        // ...and the originals are LEFT IN PLACE (never destructive).
        assert!(old.join("theme.json").is_file());
        assert!(old.join("downloads").join("song.mp3").is_file());
        // Marker written after a successful attempt.
        assert!(new.join(MIGRATION_MARKER).is_file());

        let _ = fs::remove_dir_all(&old);
        let _ = fs::remove_dir_all(&new);
    }

    #[test]
    fn migration_does_not_overwrite_existing_target_data() {
        let old = unique_test_dir("migover-old");
        let new = unique_test_dir("migover-new");
        fs::write(old.join("theme.json"), b"OLD").unwrap();
        // Target already has newer data â€” must be preserved.
        fs::write(new.join("theme.json"), b"NEWER").unwrap();

        run_migration(&old, &new).expect("migration ok");

        assert_eq!(fs::read(new.join("theme.json")).unwrap(), b"NEWER");
        let _ = fs::remove_dir_all(&old);
        let _ = fs::remove_dir_all(&new);
    }

    #[test]
    fn migration_is_skipped_once_marker_exists() {
        let old = unique_test_dir("migmark-old");
        let new = unique_test_dir("migmark-new");
        // Marker already present â†’ migration must be a no-op even with new source.
        fs::write(new.join(MIGRATION_MARKER), b"done").unwrap();
        fs::write(old.join("layout.json"), b"SHOULD-NOT-COPY").unwrap();

        run_migration(&old, &new).expect("migration ok");

        assert!(
            !new.join("layout.json").exists(),
            "marker present should skip all copying"
        );
        let _ = fs::remove_dir_all(&old);
        let _ = fs::remove_dir_all(&new);
    }

    #[test]
    fn migration_tolerates_missing_legacy_data() {
        let old = unique_test_dir("migempty-old");
        let new = unique_test_dir("migempty-new");
        // No legacy files at all â€” must not error, just write the marker.
        run_migration(&old, &new).expect("missing legacy data must not fail");
        assert!(new.join(MIGRATION_MARKER).is_file());
        let _ = fs::remove_dir_all(&old);
        let _ = fs::remove_dir_all(&new);
    }

    #[test]
    fn failed_file_copy_writes_no_marker_and_no_target() {
        let old = unique_test_dir("migfilefail-old");
        let new = unique_test_dir("migfilefail-new");
        fs::write(old.join("theme.json"), b"theme-data").unwrap();
        // Obstruct the staging temp path with a directory so the file copy fails.
        let tmp = migrate_temp_path(&new.join("theme.json"));
        fs::create_dir_all(&tmp).unwrap();

        let result = run_migration(&old, &new);
        assert!(result.is_err(), "obstructed copy must fail");
        // No marker â†’ next launch retries.
        assert!(!new.join(MIGRATION_MARKER).exists(), "no marker on failure");
        // Final target was never created from a partial copy.
        assert!(!new.join("theme.json").exists(), "no partial final target");
        // Source left intact.
        assert!(old.join("theme.json").is_file());

        let _ = fs::remove_dir_all(&old);
        let _ = fs::remove_dir_all(&new);
    }

    #[test]
    fn partial_directory_copy_leaves_no_final_target_or_marker() {
        let old = unique_test_dir("migdirfail-old");
        let new = unique_test_dir("migdirfail-new");
        let dl = old.join("downloads");
        fs::create_dir_all(&dl).unwrap();
        fs::write(dl.join("song.mp3"), b"audio").unwrap();
        // Obstruct the staging temp dir path with a FILE so the dir copy fails
        // before anything reaches the final `downloads` path.
        let tmp = migrate_temp_path(&new.join("downloads"));
        fs::write(&tmp, b"obstruction").unwrap();

        let result = run_migration(&old, &new);
        assert!(result.is_err(), "obstructed dir copy must fail");
        // The final target directory must NOT exist (so a retry isn't skipped).
        assert!(
            !new.join("downloads").exists(),
            "no partial final downloads dir"
        );
        assert!(!new.join(MIGRATION_MARKER).exists(), "no marker on failure");
        // Source left intact.
        assert!(dl.join("song.mp3").is_file());

        let _ = fs::remove_dir_all(&old);
        let _ = fs::remove_dir_all(&new);
    }
}
