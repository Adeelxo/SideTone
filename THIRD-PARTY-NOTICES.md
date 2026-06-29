# Third-Party Notices

SideTone is distributed with the following third-party programs. They are
invoked as separate, standalone executables (SideTone does not statically or
dynamically link against them); each retains its own license. When you
redistribute SideTone (for example, the installer that bundles these binaries),
you must comply with the licenses below.

SideTone's own source code is licensed separately under the MIT License — see
[LICENSE](LICENSE).

---

## FFmpeg (`ffmpeg.exe`)

- **Project:** FFmpeg — https://ffmpeg.org/
- **Bundled build:** `ffmpeg-release-essentials` from https://www.gyan.dev/ffmpeg/builds/
- **Version:** see [`assets/deps/manifest.json`](assets/deps/manifest.json)
  (`ffmpeg.exe.version`), SHA-256 pinned in the same file.
- **License:** GNU General Public License, version 3 (GPLv3).

The "essentials" build distributed with SideTone is a **GPLv3** build of FFmpeg.

In accordance with the GPL:

- The full, verbatim text of the GNU GPL v3 is included with SideTone as
  [COPYING-GPL-3.0.txt](COPYING-GPL-3.0.txt) (also installed next to the
  application) and is available online at
  https://www.gnu.org/licenses/gpl-3.0.html
- The complete corresponding source code for the exact FFmpeg build bundled with
  SideTone is available from the upstream FFmpeg project
  (https://ffmpeg.org/download.html) and from the build provider
  (https://www.gyan.dev/ffmpeg/builds/). The version that corresponds to the
  bundled binary is recorded in `assets/deps/manifest.json`.
- **Written offer:** the SideTone maintainer will, on request, provide the
  complete corresponding source for the bundled FFmpeg build for a period of
  three years. Contact: sidetone@betweensurfaces.com (also listed in
  [SECURITY.md](SECURITY.md)).

FFmpeg is a trademark of Fabrice Bellard, originator of the FFmpeg project.
SideTone is not affiliated with or endorsed by the FFmpeg project.

---

## yt-dlp (`yt-dlp.exe`)

- **Project:** yt-dlp — https://github.com/yt-dlp/yt-dlp
- **Version:** see [`assets/deps/manifest.json`](assets/deps/manifest.json)
  (`yt-dlp.exe.version`). The in-app **Repair** feature updates this binary from
  the official `yt-dlp/yt-dlp` GitHub releases.
- **License:** The Unlicense (public domain dedication) —
  https://github.com/yt-dlp/yt-dlp/blob/master/LICENSE

yt-dlp is released into the public domain under The Unlicense. No additional
redistribution restrictions apply.

---

## Bundled font — Inter

- **Project:** Inter — https://rsms.me/inter/
- **License:** SIL Open Font License 1.1 —
  https://openfontlicense.org/

Inter is bundled (`assets/fonts/Inter.ttf`) and installed for consistent
rendering. The OFL permits bundling and redistribution; the font is not sold on
its own.

---

## Rust crate dependencies

SideTone's Rust dependencies (Slint, rodio, cpal, reqwest, serde, souvlaki,
tray-icon, global-hotkey, and their transitive dependencies) are used under
their respective MIT / Apache-2.0 / BSD licenses. A full machine-generated
inventory can be produced with `cargo about` or `cargo license` from
`Cargo.lock`.
