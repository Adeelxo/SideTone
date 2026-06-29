//! Playlist import: classify Spotify/Apple links, scrape their public page
//! metadata (redirect-allowlisted), and build `artist title` search queries.
//! Pure layer extracted verbatim from `main.rs` (move-only; no logic changes).
//! The UI-bound orchestrator (`import_playlist_link`) stays in `main.rs`.

use std::time::Duration;

use crate::streaming::split_scheme_host;
use crate::AppResult;

/// A Spotify playlist/album/track link: HTTPS + exact `open.spotify.com` host +
/// a recognizable `/playlist|album|track/<id>` path. Metadata-only (we never
/// touch Spotify audio).
fn is_spotify_import_url(input: &str) -> bool {
    match split_scheme_host(input) {
        Some((scheme, host)) if scheme == "https" && host == "open.spotify.com" => {
            spotify_kind_id(&input.trim().to_ascii_lowercase()).is_some()
        }
        _ => false,
    }
}

/// An Apple Music album/playlist/song link: HTTPS + exact `music.apple.com` host
/// + a recognizable path segment. Metadata-only (we never touch Apple audio).
fn is_apple_import_url(input: &str) -> bool {
    match split_scheme_host(input) {
        Some((scheme, host)) if scheme == "https" && host == "music.apple.com" => {
            let v = input.trim().to_ascii_lowercase();
            v.contains("/album/") || v.contains("/playlist/") || v.contains("/song/")
        }
        _ => false,
    }
}

pub(crate) fn is_playlist_import_url(input: &str) -> bool {
    is_spotify_import_url(input) || is_apple_import_url(input)
}

/// Redirect-host allowlist for the playlist-import scraper. The only sites we
/// ever scrape are Spotify and Apple Music, so we only follow redirects to hosts
/// those services own. This stops a hijacked/MITM'd redirect from steering the
/// fetch to an attacker-controlled page whose HTML would then become `ytsearch`
/// queries. Suffix-matched on the registrable domain (with a leading dot) so a
/// spoof like `spotify.com.evil.com` is rejected. Pure + unit-tested.
fn is_import_allowed_redirect_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    const OWNED: [&str; 4] = ["spotify.com", "spotifycdn.com", "apple.com", "mzstatic.com"];
    OWNED
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

fn http_get_text(url: &str) -> AppResult<String> {
    let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() > 10 {
            return attempt.error("too many redirects");
        }
        match attempt.url().host_str() {
            Some(host) if is_import_allowed_redirect_host(host) => attempt.follow(),
            _ => attempt.error("import redirect to a non-Spotify/Apple host was blocked"),
        }
    });
    let client = reqwest::blocking::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .timeout(Duration::from_secs(20))
        .redirect(redirect_policy)
        .build()?;
    let resp = client.get(url).send()?;
    if !resp.status().is_success() {
        return Err(format!("page returned HTTP {}", resp.status().as_u16()).into());
    }
    Ok(resp.text()?)
}

/// Pull the inner text of a `<script ... id="ID" ...>...</script>` block.
fn extract_tag_json(body: &str, id: &str) -> Option<String> {
    let markers = [
        format!("id=\"{id}\""),
        format!("id='{id}'"),
        format!("id={id}"),
    ];
    let idx = markers
        .iter()
        .filter_map(|marker| body.find(marker))
        .min()?;
    let after = &body[idx..];
    let start = after.find('>')? + 1;
    let end = after[start..].find("</script>")?;
    Some(after[start..start + end].trim().to_string())
}

/// "artist title" search queries for each track in a Spotify / Apple link.
pub(crate) fn fetch_playlist_tracks(url: &str) -> AppResult<Vec<String>> {
    let lower = url.to_ascii_lowercase();
    if lower.contains("open.spotify.com/") {
        fetch_spotify_tracks(url)
    } else if lower.contains("music.apple.com/") {
        fetch_apple_tracks(url)
    } else {
        Err("Unsupported link.".into())
    }
}

fn spotify_kind_id(url: &str) -> Option<(String, String)> {
    for kind in ["playlist", "album", "track"] {
        let pat = format!("/{kind}/");
        if let Some(pos) = url.find(&pat) {
            let rest = &url[pos + pat.len()..];
            let id: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if !id.is_empty() {
                return Some((kind.to_string(), id));
            }
        }
    }
    None
}

fn fetch_spotify_tracks(url: &str) -> AppResult<Vec<String>> {
    let (kind, id) = spotify_kind_id(url).ok_or("Not a Spotify playlist, album, or track link.")?;
    // The embed page ships the track list as JSON without needing API credentials.
    let embed = format!("https://open.spotify.com/embed/{kind}/{id}");
    let body = http_get_text(&embed)?;
    let json = extract_tag_json(&body, "__NEXT_DATA__")
        .ok_or("Couldn't read the Spotify page (its format may have changed).")?;
    let value: serde_json::Value = serde_json::from_str(&json)?;
    let mut out = Vec::new();
    collect_spotify_tracks(&value, &mut out);
    dedupe_import_queries(&mut out);
    if out.is_empty() {
        return Err("Spotify did not expose any tracks on that page.".into());
    }
    Ok(out)
}

// Spotify has changed the public embed payload a few times. Handle the old
// `trackList` shape plus newer nested `track` objects with `name`/`artists`.
fn collect_spotify_tracks(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Array(list)) = map.get("trackList") {
                for item in list {
                    if let Some(title) = item.get("title").and_then(|t| t.as_str()) {
                        let artist = item.get("subtitle").and_then(|s| s.as_str()).unwrap_or("");
                        push_import_query(out, artist, title);
                    }
                }
            }

            if looks_like_spotify_track(map) {
                if let Some(title) = map
                    .get("name")
                    .or_else(|| map.get("title"))
                    .and_then(|t| t.as_str())
                {
                    let artist = spotify_artist_name(map);
                    push_import_query(out, artist.as_deref().unwrap_or(""), title);
                }
            }

            for (key, child) in map {
                if key == "track" {
                    match child {
                        serde_json::Value::Object(track) => {
                            if let Some(title) = track
                                .get("name")
                                .or_else(|| track.get("title"))
                                .and_then(|t| t.as_str())
                            {
                                let artist = spotify_artist_name(track);
                                push_import_query(out, artist.as_deref().unwrap_or(""), title);
                            }
                        }
                        serde_json::Value::Array(list) => {
                            for item in list {
                                collect_spotify_tracks(item, out);
                            }
                        }
                        _ => {}
                    }
                }
            }

            for child in map.values() {
                collect_spotify_tracks(child, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for child in arr {
                collect_spotify_tracks(child, out);
            }
        }
        _ => {}
    }
}

fn looks_like_spotify_track(map: &serde_json::Map<String, serde_json::Value>) -> bool {
    map.contains_key("artists")
        || map.contains_key("duration_ms")
        || map.contains_key("preview_url")
        || map.contains_key("audioPreview")
        || map
            .get("type")
            .and_then(|t| t.as_str())
            .map(|kind| kind.eq_ignore_ascii_case("track"))
            .unwrap_or(false)
}

fn spotify_artist_name(map: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    if let Some(artists) = map.get("artists") {
        return artist_names_from_json(artists);
    }
    map.get("subtitle")
        .or_else(|| map.get("artistName"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

fn fetch_apple_tracks(url: &str) -> AppResult<Vec<String>> {
    let body = http_get_text(url)?;
    let mut out = Vec::new();
    // Apple Music album pages carry a schema.org ld+json block with the tracks.
    let mut search = body.as_str();
    while let Some(pos) = search.find("application/ld+json") {
        let after = &search[pos..];
        if let Some(start_rel) = after.find('>') {
            let start = start_rel + 1;
            if let Some(end) = after[start..].find("</script>") {
                let json = after[start..start + end].trim();
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
                    collect_apple_tracks(&value, &mut out);
                }
                search = &after[start + end..];
                continue;
            }
        }
        break;
    }
    if out.is_empty() {
        if let Some(json) = extract_tag_json(&body, "serialized-server-data") {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
                collect_apple_tracks(&value, &mut out);
            }
        }
    }
    dedupe_import_queries(&mut out);
    if out.is_empty() {
        return Err("Apple Music did not expose any tracks on that page.".into());
    }
    Ok(out)
}

// schema.org MusicRecording entries carry "name" and (often) byArtist.name.
fn collect_apple_tracks(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            let is_track = map
                .get("@type")
                .and_then(|t| t.as_str())
                .map(|t| t == "MusicRecording" || t == "Song")
                .unwrap_or(false);
            if is_track {
                if let Some(name) = map.get("name").and_then(|n| n.as_str()) {
                    let artist = map.get("byArtist").and_then(artist_names_from_json);
                    push_import_query(out, artist.as_deref().unwrap_or(""), name);
                }
            }

            if let Some(name) = map.get("name").and_then(|n| n.as_str()) {
                if map.contains_key("artistName") || map.contains_key("artist") {
                    let artist = map
                        .get("artistName")
                        .or_else(|| map.get("artist"))
                        .and_then(|a| a.as_str())
                        .unwrap_or("");
                    push_import_query(out, artist, name);
                }
            }

            if is_apple_track_lockup(map) {
                if let Some(title) = map.get("title").and_then(|n| n.as_str()) {
                    let artist = map
                        .get("artistName")
                        .or_else(|| map.get("artist"))
                        .and_then(|a| a.as_str())
                        .or_else(|| {
                            map.get("subtitleLinks")
                                .and_then(|links| links.as_array())
                                .and_then(|links| links.first())
                                .and_then(|first| first.get("title"))
                                .and_then(|title| title.as_str())
                        })
                        .unwrap_or("");
                    push_import_query(out, artist, title);
                }
            }

            for child in map.values() {
                collect_apple_tracks(child, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for child in arr {
                collect_apple_tracks(child, out);
            }
        }
        _ => {}
    }
}

fn is_apple_track_lockup(map: &serde_json::Map<String, serde_json::Value>) -> bool {
    map.get("id")
        .and_then(|id| id.as_str())
        .map(|id| id.starts_with("track-lockup"))
        .unwrap_or(false)
        || (map.contains_key("artistName") && map.contains_key("duration"))
        || map
            .get("impressionMetrics")
            .and_then(|metrics| metrics.get("fields"))
            .and_then(|fields| fields.get("kind"))
            .and_then(|kind| kind.as_str())
            .map(|kind| kind == "song")
            .unwrap_or(false)
}

fn artist_names_from_json(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Object(map) => map
            .get("name")
            .or_else(|| map.get("artistName"))
            .and_then(|name| name.as_str())
            .map(|name| name.to_string()),
        serde_json::Value::Array(list) => {
            let names = list
                .iter()
                .filter_map(artist_names_from_json)
                .filter(|name| !name.trim().is_empty())
                .collect::<Vec<_>>();
            if names.is_empty() {
                None
            } else {
                Some(names.join(" "))
            }
        }
        _ => None,
    }
}

fn push_import_query(out: &mut Vec<String>, artist: &str, title: &str) {
    let title = clean_import_text(title);
    if title.is_empty() {
        return;
    }
    let artist = clean_import_text(artist);
    let query = if artist.is_empty() {
        title
    } else {
        format!("{artist} {title}")
    };
    if !out
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(&query))
    {
        out.push(query);
    }
}

fn clean_import_text(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn dedupe_import_queries(tracks: &mut Vec<String>) {
    let mut deduped = Vec::new();
    for track in tracks.drain(..) {
        let track = clean_import_text(&track);
        if !track.is_empty()
            && !deduped
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(&track))
        {
            deduped.push(track);
        }
    }
    *tracks = deduped;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_spotify_and_apple_import_urls_are_accepted() {
        assert!(is_spotify_import_url(
            "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"
        ));
        assert!(is_spotify_import_url(
            "https://open.spotify.com/album/1DFixLWuPkv3KT3TnV35m3"
        ));
        assert!(is_spotify_import_url(
            "https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT"
        ));
        assert!(is_apple_import_url(
            "https://music.apple.com/us/album/some-album/1440857781"
        ));
        assert!(is_apple_import_url(
            "https://music.apple.com/us/playlist/chill/pl.abc123"
        ));
        // Routed through the combined gate too.
        assert!(is_playlist_import_url(
            "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"
        ));
    }

    #[test]
    fn spoofed_or_http_spotify_apple_urls_are_rejected() {
        for url in [
            "http://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M", // http
            "https://open.spotify.com.evil.com/playlist/abc",          // host spoof
            "https://evil.com/?u=open.spotify.com/playlist/abc",       // substring spoof
            "https://open.spotify.com@evil.com/playlist/abc",          // userinfo spoof
            "https://open.spotify.com/profile/xyz",                    // unsupported kind
            "http://music.apple.com/us/album/x/1",                     // http
            "https://music.apple.com.evil.com/us/album/x/1",           // host spoof
            "https://music.apple.com/us/artist/x/1",                   // unsupported path
        ] {
            assert!(!is_playlist_import_url(url), "should reject: {url}");
        }
    }

    #[test]
    fn import_redirects_follow_only_spotify_and_apple_owned_hosts() {
        // Spotify + Apple owned hosts (incl. their CDNs) are followed.
        for host in [
            "open.spotify.com",
            "spotify.com",
            "i.scdn.co.spotifycdn.com",
            "music.apple.com",
            "apple.com",
            "is1-ssl.mzstatic.com",
            "SPOTIFY.COM",       // case-insensitive
            "open.spotify.com.", // trailing dot
        ] {
            assert!(
                is_import_allowed_redirect_host(host),
                "should allow: {host}"
            );
        }
        // Anything else — including suffix spoofs — is blocked.
        for host in [
            "evil.com",
            "spotify.com.evil.com",
            "apple.com.evil.com",
            "notspotify.com",
            "mzstatic.com.evil.com",
            "github.com",
        ] {
            assert!(
                !is_import_allowed_redirect_host(host),
                "should block: {host}"
            );
        }
    }
}
