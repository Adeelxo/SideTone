# SideTone

A lightweight desktop music player for Windows with **Focus mode** — it softens
distracting parts of the mix so audio sits behind what you're doing instead of
fighting for attention. Local playback, playlists, downloads, and self-repairing
YouTube streaming, in a small, fast, native app (Slint + Rust).

## What it is

- A small native Rust app (no browser engine, no background service, **no
  telemetry, no analytics, no database**). All settings, playlists, and downloads
  stay on your machine.
- Streaming uses bundled `yt-dlp.exe` + `ffmpeg.exe` helpers, invoked as separate
  executables. Local playback is fully independent of the network.

## Build

Requires the Rust toolchain.

```powershell
cargo build --release      # builds target\release\sidetone.exe
cargo test                 # unit tests
```

The streaming helpers (`yt-dlp.exe`, `ffmpeg.exe`) are fetched per
`assets/deps/manifest.json` (verified by SHA-256) and bundled into the installer,
which is built from `installer.iss` with [Inno Setup](https://jrsoftware.org/isdl.php).

## Privacy & network use

SideTone collects nothing and phones home for nothing. It makes outbound
connections only for features you actively use:

- **Streaming / search** — `yt-dlp` contacts YouTube to search and fetch audio.
- **Playlist import** — pasting a Spotify/Apple Music link reads that service's
  public page for the track list (titles/artists only; redirects restricted to
  those services' hosts), then resolves each track via YouTube search.
- **Update check** — reads the latest version number from the GitHub Releases API
  to show a banner; never auto-downloads or runs anything.
- **Streaming Repair** — downloads a fresh `yt-dlp` from the official
  `yt-dlp/yt-dlp` GitHub release (HTTPS, GitHub-owned hosts only, size-bounded,
  self-tested, atomic swap with rollback).

SideTone only ever deletes files inside its own managed `downloads/` folder;
scanned library files are never modified or deleted.

## License & third-party software

SideTone's own source code is licensed under the **MIT License** — see
[LICENSE](LICENSE).

The installer bundles third-party programs invoked as separate executables (not
linked): **FFmpeg** (GPLv3) and **yt-dlp** (Unlicense), plus the **Inter** font
(SIL OFL 1.1). If you redistribute SideTone you must honor these — in particular
FFmpeg's GPL source offer. See [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md);
the verbatim GPLv3 text is in [COPYING-GPL-3.0.txt](COPYING-GPL-3.0.txt).

## Security

To report a vulnerability, see [SECURITY.md](SECURITY.md).
