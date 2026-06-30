use std::time::Duration;

// Only used by the Windows `explorer` branch in `open_validated_url`; gating the
// import keeps non-Windows builds warning-clean.
#[cfg(windows)]
use crate::streaming_helpers::hidden_command;

const RELEASES_API: &str = "https://api.github.com/repos/Adeelxo/SideTone/releases/latest";
const UPDATE_PAGE_URL: &str = "https://github.com/Adeelxo/SideTone/releases";

pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let start = s.find(|c: char| c.is_ascii_digit())?;
    let token: String = s[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = token.split('.').filter_map(|p| p.parse::<u32>().ok());
    let major = parts.next()?;
    Some((major, parts.next().unwrap_or(0), parts.next().unwrap_or(0)))
}

pub fn fetch_latest_release() -> Option<(String, (u32, u32, u32))> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("SideTone/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let resp = client.get(RELEASES_API).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_str(&resp.text().ok()?).ok()?;
    let tag = json.get("tag_name")?.as_str()?.to_string();
    let version = parse_version(&tag)?;
    Some((tag, version))
}

pub fn open_update_page() {
    open_validated_url(UPDATE_PAGE_URL);
}

fn open_validated_url(url: &str) {
    if !is_allowed_update_url(url) {
        #[cfg(debug_assertions)]
        eprintln!("update: refused to open disallowed URL: {url}");
        return;
    }
    #[cfg(windows)]
    {
        let _ = hidden_command("explorer").arg(url).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

fn is_allowed_update_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if !host.is_ascii() || host.contains("xn--") {
        return false;
    }
    if host != "github.com" {
        return false;
    }
    if path.contains('%') || path.contains("..") || path.contains('\\') {
        return false;
    }
    if path == "/Adeelxo/SideTone/releases" {
        return true;
    }
    if let Some(tag) = path.strip_prefix("/Adeelxo/SideTone/releases/tag/") {
        return !tag.is_empty()
            && !tag.contains('/')
            && tag
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_');
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_url_allowlist_accepts_only_releases_and_tags() {
        assert!(is_allowed_update_url(UPDATE_PAGE_URL));
        assert!(is_allowed_update_url(
            "https://github.com/Adeelxo/SideTone/releases"
        ));
        assert!(is_allowed_update_url(
            "https://github.com/Adeelxo/SideTone/releases/tag/sidetone-v6.1"
        ));

        assert!(!is_allowed_update_url(
            "http://github.com/Adeelxo/SideTone/releases"
        ));
        assert!(!is_allowed_update_url(
            "https://github.com.evil.com/Adeelxo/SideTone/releases"
        ));
        assert!(!is_allowed_update_url(
            "https://github.com@evil.com/Adeelxo/SideTone/releases"
        ));
        assert!(!is_allowed_update_url(
            "https://GitHub.com/Adeelxo/SideTone/releases"
        ));
        assert!(!is_allowed_update_url(
            "https://g\u{0456}thub.com/Adeelxo/SideTone/releases"
        ));
        assert!(!is_allowed_update_url(
            "https://xn--gthub-l1a.com/Adeelxo/SideTone/releases"
        ));

        assert!(!is_allowed_update_url(
            "https://github.com/evil/repo/releases"
        ));
        assert!(!is_allowed_update_url(
            "https://github.com/Adeelxo/SideTone/issues"
        ));

        assert!(!is_allowed_update_url("javascript:alert(1)"));
        assert!(!is_allowed_update_url("file:///etc/passwd"));
        assert!(!is_allowed_update_url("shell:Startup"));
        assert!(!is_allowed_update_url("cmd:/c calc"));
        assert!(!is_allowed_update_url("powershell:-c calc"));

        assert!(!is_allowed_update_url(
            "https://github.com/Adeelxo/SideTone/releases/../../evil"
        ));
        assert!(!is_allowed_update_url(
            "https://github.com/Adeelxo/SideTone/releases/tag/..%2f..%2fevil"
        ));
        assert!(!is_allowed_update_url(
            "https://github.com/Adeelxo/SideTone/releases/tag/a%2Fb"
        ));
        assert!(!is_allowed_update_url(
            "https://github.com/Adeelxo/SideTone/releases/tag/"
        ));
        assert!(!is_allowed_update_url(
            "https://github.com/Adeelxo/SideTone/releases/tag/v1/extra"
        ));
    }

    #[test]
    fn version_parsing_is_tag_format_tolerant() {
        assert_eq!(parse_version("6.1.0"), Some((6, 1, 0)));
        assert_eq!(parse_version("v6.1"), Some((6, 1, 0)));
        assert_eq!(parse_version("sidetone-v6.1"), Some((6, 1, 0)));
        assert!(parse_version("no-digits-here").is_none());
        assert!(parse_version("sidetone-v7.0").unwrap() > (6, 1, 0));
    }
}
