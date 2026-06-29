//! Streaming helpers (pure): YouTube URL classification/allowlist, the yt-dlp
//! search/URL target, and yt-dlp error-message summarization. Extracted verbatim
//! from `main.rs` (move-only; no logic changes). Audio-pipeline spawns and
//! playback orchestration stay in `main.rs` for the later player split.

use std::error::Error;
use std::process::Output;

use crate::{truncate, AppResult};

/// Split a URL into `(scheme, host)` with the host lowercased and userinfo/port
/// stripped â€” small, dependency-free, and good enough for exact-host allowlists.
/// Crucially it returns the REAL host, so spoofs like
/// `https://youtube.com.evil.com/...`, `https://youtube.com@evil.com/...`, and
/// `https://evil.com/?q=youtube.com` resolve to `evil.com` and get rejected.
/// Returns None for anything without a `scheme://host`.
pub(crate) fn split_scheme_host(input: &str) -> Option<(String, String)> {
    let trimmed = input.trim();
    let (scheme, rest) = trimmed.split_once("://")?;
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
    {
        return None;
    }
    // The authority is everything up to the first '/', '?', or '#'.
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return None;
    }
    // Drop userinfo (`user:pass@host`) â€” the host is after the LAST '@'.
    let host_port = match authority.rsplit_once('@') {
        Some((_userinfo, hp)) => hp,
        None => authority,
    };
    // Drop a port. (We don't expect bracketed IPv6 literals for these hosts.)
    let host = host_port.split(':').next().unwrap_or(host_port);
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    Some((scheme.to_ascii_lowercase(), host))
}

/// Exact-host allowlist for YouTube. No substrings, so a host has to BE one of
/// these â€” not merely contain "youtube.com".
fn is_youtube_host(host: &str) -> bool {
    matches!(
        host,
        "youtube.com" | "www.youtube.com" | "music.youtube.com" | "m.youtube.com" | "youtu.be"
    )
}

/// A directly-pasted YouTube URL: HTTPS + an exact YouTube host. Plain search
/// text (no `scheme://host`) returns false, so it stays a search.
pub(crate) fn is_youtube_url(input: &str) -> bool {
    match split_scheme_host(input) {
        Some((scheme, host)) => scheme == "https" && is_youtube_host(&host),
        None => false,
    }
}

pub(crate) fn yt_target(input: &str) -> String {
    if is_youtube_url(input) {
        input.trim().to_string()
    } else {
        format!("ytsearch1:{input}")
    }
}

pub(crate) fn ensure_success(label: &str, output: &Output) -> AppResult<()> {
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(summarize_ytdlp_failure(label, &stderr).into())
}

pub(crate) fn error_status_text(error: &dyn Error) -> String {
    let summary = summarize_external_error(&error.to_string());
    format!("Error: {}", truncate_single_line(&summary, 160))
}

fn summarize_external_error(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("too many requests") || lower.contains("http error 429") {
        return "YouTube is rate-limiting requests right now. Wait a bit, then try again."
            .to_string();
    }
    if lower.contains("you should update") || lower.contains("run yt-dlp -u") {
        return "SideTone's streaming engine is outdated. Open Settings and click Repair to update it automatically.".to_string();
    }
    if lower.contains("timed out") {
        return "The streaming helper took too long to respond. Check the connection and try again.".to_string();
    }
    if lower.contains("sign in to confirm")
        || lower.contains("not a bot")
        || lower.contains("confirm you're not a bot")
    {
        return "YouTube blocked this request with a bot-check. Try again later or use a downloaded/local track.".to_string();
    }
    if lower.contains("yt-dlp is not installed") {
        return "yt-dlp is missing. Reinstall SideTone or place yt-dlp.exe next to the app."
            .to_string();
    }
    if lower.contains("ffmpeg") && lower.contains("not found") {
        return "ffmpeg is missing. Reinstall SideTone or place ffmpeg.exe next to the app."
            .to_string();
    }
    if lower.contains("unable to download webpage") {
        return "Could not reach YouTube. Check the connection and try again.".to_string();
    }
    strip_ytdlp_noise(raw)
}

fn summarize_ytdlp_failure(label: &str, stderr: &str) -> String {
    let summary = summarize_external_error(stderr);
    if summary.is_empty() {
        format!("{label} failed.")
    } else {
        format!("{label}: {summary}")
    }
}

fn strip_ytdlp_noise(raw: &str) -> String {
    let useful = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            !lower.starts_with("warning:")
                && !lower.contains("you should update")
                && !lower.contains("run yt-dlp -u")
        })
        .collect::<Vec<_>>();
    if useful.is_empty() {
        "YouTube request failed. Try again in a moment.".to_string()
    } else {
        useful.join(" ")
    }
}

fn truncate_single_line(value: &str, max_chars: usize) -> String {
    let single_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&single_line, max_chars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn ytdlp_errors_are_summarized_for_status_bar() {
        let rate_limited = summarize_ytdlp_failure(
            "yt-dlp metadata",
            "WARNING: You should update\nERROR: [youtube] abc: HTTP Error 429: Too Many Requests",
        );
        assert!(rate_limited.contains("rate-limiting"));
        assert!(!rate_limited.contains("WARNING"));

        let bot_check = summarize_ytdlp_failure(
            "yt-dlp metadata",
            "ERROR: [youtube] xyz: Sign in to confirm you're not a bot",
        );
        assert!(bot_check.contains("bot-check"));

        let noisy_error =
            io::Error::other("WARNING: update available\nERROR: useful short failure");
        let noisy = error_status_text(&noisy_error);
        assert!(noisy.contains("useful short failure"));
        assert!(!noisy.contains("update available"));
    }

    #[test]
    fn valid_youtube_hosts_are_accepted() {
        for url in [
            "https://youtube.com/watch?v=abc123",
            "https://www.youtube.com/watch?v=abc123",
            "https://music.youtube.com/watch?v=abc123",
            "https://m.youtube.com/watch?v=abc123",
            "https://youtu.be/abc123",
            "  https://www.youtube.com/watch?v=abc123  ", // trimmed
            "HTTPS://WWW.YOUTUBE.COM/watch?v=abc123",     // case-insensitive
        ] {
            assert!(is_youtube_url(url), "should accept: {url}");
        }
    }

    #[test]
    fn spoofed_youtube_hosts_are_rejected() {
        for url in [
            "https://evil.com/?q=youtube.com",
            "https://youtube.com.evil.com/watch?v=abc",
            "https://github.com/youtube.com/watch?v=abc",
            "https://youtube.com@evil.com/watch?v=abc",
            "https://notyoutube.com/watch?v=abc",
            "https://youtube.com.co/watch?v=abc",
            "https://fakeyoutu.be/abc",
        ] {
            assert!(!is_youtube_url(url), "should reject: {url}");
        }
    }

    #[test]
    fn http_youtube_is_rejected() {
        assert!(!is_youtube_url("http://youtube.com/watch?v=abc"));
        assert!(!is_youtube_url("http://www.youtube.com/watch?v=abc"));
        assert!(!is_youtube_url("http://youtu.be/abc"));
    }
}
