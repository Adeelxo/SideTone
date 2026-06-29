# Security Policy

## Reporting a vulnerability

If you find a security issue in SideTone, please report it privately rather than
opening a public issue:

- **Email:** sidetone@betweensurfaces.com
- Please include steps to reproduce, the affected version, and the impact you
  observed. A proof-of-concept is helpful but not required.

You can expect an acknowledgement within a reasonable time. SideTone is a small
project maintained by a single developer, so there is **no bug-bounty program**,
but credible reports are taken seriously and fixed as a priority.

Please do not disclose the issue publicly until a fix has shipped.

## Scope

In scope:

- The SideTone application code (`src/`).
- The in-app streaming-engine **Repair** / update flow.
- The installer and release/signing process.

Out of scope (report upstream):

- Vulnerabilities in the bundled helper programs themselves — **FFmpeg**
  (https://ffmpeg.org/security.html) and **yt-dlp**
  (https://github.com/yt-dlp/yt-dlp/security). SideTone tracks upstream yt-dlp
  via Repair.

## Security model (summary)

- **No accounts, no telemetry, no analytics, no remote database.** All user data
  stays on the local machine.
- **Network use is limited and disclosed** — see "Privacy & network use" in the
  [README](README.md#privacy--network-use).
- **Helper updates are constrained:** Repair downloads yt-dlp only from the
  official `yt-dlp/yt-dlp` GitHub release over HTTPS, follows redirects only to
  GitHub-owned hosts, enforces a download size bound, runs a `--version`
  self-test, and swaps the binary atomically with rollback. A failed update
  never replaces the working helper.
- **File deletion is bounded:** SideTone only deletes files inside its own
  managed `downloads/` folder (verified by canonicalized path containment).
  Scanned library files are never modified or deleted; removing a library or
  playlist entry only edits SideTone's own index/metadata.
