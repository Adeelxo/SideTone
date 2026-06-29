# SideTone streaming helpers

Place verified release copies of these files here before building the installer:

- `yt-dlp.exe`
- `ffmpeg.exe`
- `manifest.json`

The release build blocks installer creation if either helper is missing or its
SHA256 doesn't match `manifest.json`. The installer bundles these files next to
`sidetone.exe`, and the app prefers that local copy over anything on `PATH`.

Do not rely on users to install or update these tools manually — the installer is
self-contained. For official releases, refresh these helpers as part of the
release process and keep the source URL, version, and checksum in `manifest.json`
(used only at build time to verify the bundled bytes).

## In-app Repair model (v7)

Repair is one-click and in-app — users never download a new installer (or any
helper file) for normal `yt-dlp` breakage. Repair updates **only `yt-dlp.exe`**
(ffmpeg is bundled-only and never updated this way). The flow is:

1. **Source.** Repair downloads the **latest official yt-dlp** directly from the
   yt-dlp project's GitHub release:
   `https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe`.
   This exact URL is the only accepted initial source (HTTPS, exact host + path).
2. **Redirects.** GitHub's `latest/download` hops to its asset CDN; redirects are
   followed **only to GitHub-owned hosts** (`github.com` / `*.githubusercontent.com`).
3. **Size guard.** The download must be within sane bounds (≥ 1 MB, ≤ 64 MB) or
   it's rejected.
4. **Liveness gate (`--version`).** The downloaded binary must run and print a
   plausible yt-dlp version before it is trusted.
5. **Atomic swap + rollback.** It's swapped in with a `.old` backup; a failed
   download / size / self-test / swap always leaves the previously working helper
   in place, and local/downloaded playback keeps working even if Repair fails.

> Trust model: HTTPS + the exact official yt-dlp URL + GitHub-owned redirect hosts
> + size bounds + a `--version` self-test — direct official yt-dlp GitHub latest
> with SideTone's local validation gates. **The developer does NOT host or upload
> any helper manifest / yt-dlp release asset for normal Repair.**

### Release process

For each official release, refresh the bundled helpers here so a fresh install
ships a recent yt-dlp/ffmpeg, and keep `manifest.json` in sync (it's the
**build-time** checksum gate, not used by Repair):

```powershell
Get-FileHash -Algorithm SHA256 assets\deps\yt-dlp.exe
Get-FileHash -Algorithm SHA256 assets\deps\ffmpeg.exe
```
