use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub struct Track {
    pub path: PathBuf,
}

#[derive(Debug)]
pub struct Collection {
    pub name: String,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct YtEntry {
    pub title: Option<String>,
    pub webpage_url: Option<String>,
    pub url: Option<String>,
    pub duration: Option<f64>,
    pub entries: Option<Vec<YtEntry>>,
}

#[derive(Default, Clone)]
pub enum LocalKind {
    #[default]
    None,
    Library,
    Downloads,
    Playlist(String),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum QueueContext {
    #[default]
    Stream,
    Library,
    Downloads,
    Playlist(String),
}

#[derive(Clone, Copy)]
pub enum QueueDirection {
    Previous,
    Next,
}

pub struct QueuedTrack {
    pub title: String,
    pub url: String,
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct YtResultSlot {
    pub title: String,
    pub url: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub tracks: Vec<YtResultSlot>,
}

#[derive(Default)]
pub struct AppQueue {
    // Playback queue (what prev/next walks through)
    pub results: Vec<YtResultSlot>,
    pub current_index: Option<usize>,
    // Per-tab display lists - kept separate so Local never bleeds into YouTube
    pub yt_items: Vec<YtResultSlot>,
    pub local_items: Vec<YtResultSlot>,
    // What the Local list currently represents (library, downloads, or a playlist)
    pub local_kind: LocalKind,
    // URL of the track currently playing (drives the highlight in any list)
    pub now_playing_url: String,
    // Monotonic "play request" counter. Bumped each time the user (or auto-
    // advance) starts a new track; a background resolver that finishes after a
    // newer request was issued is stale and must not commit audio or repaint.
    pub play_gen: u64,
    // Same idea as play_gen, but for YouTube searches. A slower previous search
    // must not repaint the result panel after a newer search or tab switch.
    pub search_gen: u64,
    // The list that owns the current playback queue. Used to avoid auto-advance
    // repainting Library while the user is looking at Downloads/a playlist, or vice versa.
    pub playback_context: QueueContext,
    // Shuffle toggle backups, per visible list: Some((original, shuffled)) while
    // shuffle is active. Deactivating restores the original order. Kept separate
    // for Stream (yt) and Local so each tab toggles independently.
    pub yt_shuffle: Option<(Vec<YtResultSlot>, Vec<YtResultSlot>)>,
    pub local_shuffle: Option<(Vec<YtResultSlot>, Vec<YtResultSlot>)>,
}

impl AppQueue {
    // Reserve the next play-request generation; the caller hands this token to
    // its background worker, which re-checks it before committing.
    pub fn bump_play_gen(&mut self) -> u64 {
        self.play_gen = self.play_gen.wrapping_add(1);
        self.play_gen
    }

    pub fn bump_search_gen(&mut self) -> u64 {
        self.search_gen = self.search_gen.wrapping_add(1);
        self.search_gen
    }

    pub fn clear(&mut self) {
        self.results.clear();
        self.current_index = None;
    }

    // Start playing `url` from `list`: that list becomes the playback queue.
    pub fn play_from(&mut self, list: Vec<YtResultSlot>, url: &str) {
        self.play_from_context(list, url, QueueContext::Stream);
    }

    pub fn play_from_context(&mut self, list: Vec<YtResultSlot>, url: &str, context: QueueContext) {
        self.results = list;
        self.current_index = self.results.iter().position(|r| r.url == url);
        self.now_playing_url = url.to_string();
        self.playback_context = context;
    }

    pub fn select_index(&mut self, index: usize) -> Option<QueuedTrack> {
        let track = self.track_at(index)?;
        self.current_index = Some(index);
        Some(track)
    }

    pub fn select_neighbor(&mut self, direction: QueueDirection) -> Option<QueuedTrack> {
        let current = self.current_index?;
        let next = match direction {
            QueueDirection::Previous => current.checked_sub(1)?,
            QueueDirection::Next => current + 1,
        };

        let track = self.track_at(next)?;
        self.current_index = Some(next);
        Some(track)
    }

    fn track_at(&self, index: usize) -> Option<QueuedTrack> {
        let result = self.results.get(index)?;
        if result.url.is_empty() {
            return None;
        }

        Some(QueuedTrack {
            title: result.title.clone(),
            url: result.url.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(title: &str, url: &str) -> YtResultSlot {
        YtResultSlot {
            title: title.to_string(),
            url: url.to_string(),
        }
    }

    #[test]
    fn stream_playback_queue_keeps_stream_context() {
        let mut queue = AppQueue {
            local_items: vec![slot("download", "C:/Music/download.mp3")],
            local_kind: LocalKind::Downloads,
            ..Default::default()
        };
        let stream = vec![
            slot("one", "https://youtu.be/one"),
            slot("two", "https://youtu.be/two"),
        ];

        queue.play_from(stream, "https://youtu.be/one");

        assert_eq!(queue.playback_context, QueueContext::Stream);
        assert_eq!(queue.results.len(), 2);
        assert_eq!(queue.local_items.len(), 1);
        assert_eq!(
            queue.select_neighbor(QueueDirection::Next).unwrap().url,
            "https://youtu.be/two"
        );
    }

    #[test]
    fn local_playback_queue_keeps_its_local_context() {
        let mut queue = AppQueue::default();
        let downloads = vec![
            slot("download one", "C:/Music/one.mp3"),
            slot("download two", "C:/Music/two.mp3"),
        ];

        queue.play_from_context(downloads, "C:/Music/one.mp3", QueueContext::Downloads);

        assert_eq!(queue.playback_context, QueueContext::Downloads);
        assert_eq!(
            queue.select_neighbor(QueueDirection::Next).unwrap().url,
            "C:/Music/two.mp3"
        );
    }
}
