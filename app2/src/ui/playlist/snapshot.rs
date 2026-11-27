use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, PoisonError},
};

use crate::{
    music::library::{PlaylistReceipt, SavedPlaylist, playlist_snapshot_fresh},
    playback::can_resolve,
    playlist::{ArtistCredit, Playlist, PlaylistKey, PlaylistTrack},
    providers::ProviderId,
};

#[cfg(test)]
mod tests;

const MAX_CACHED_PLAYLISTS: usize = 8;
const MAX_CACHED_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct Snapshot {
    pub(super) playlist: Playlist,
    pub(super) revision: Option<u64>,
    pub(super) tokens: Vec<SearchText>,
    pub(super) duration: Option<u64>,
    pub(super) cover_candidates: Vec<String>,
    pub(super) has_audio: bool,
    saved: Arc<Mutex<Option<PlaylistReceipt>>>,
    cache_bytes: usize,
}

#[derive(Clone)]
pub(super) struct SearchText {
    pub(super) title: String,
    pub(super) artist: String,
    pub(super) album: String,
}

impl Snapshot {
    pub(super) fn prepare(
        playlist: Playlist,
        revision: Option<u64>,
        key: &PlaylistKey,
    ) -> Arc<Self> {
        Self::build(playlist, revision, key, None, 0)
    }

    fn build(
        playlist: Playlist,
        revision: Option<u64>,
        key: &PlaylistKey,
        saved: Option<PlaylistReceipt>,
        encoded_bytes: usize,
    ) -> Arc<Self> {
        let tokens: Vec<_> = playlist
            .tracks
            .iter()
            .map(|track| SearchText {
                title: track.title.to_lowercase(),
                artist: track.artist.to_lowercase(),
                album: track.album.as_deref().unwrap_or_default().to_lowercase(),
            })
            .collect();
        let duration = playlist.tracks.iter().try_fold(0, |sum, track| {
            track.duration.map(|duration| sum + u64::from(duration))
        });
        let cover_candidates = super::artwork::cover_candidates(&playlist.tracks);
        let has_audio = playlist.tracks.iter().any(|track| can_resolve(key, track));
        let search_strings = tokens
            .iter()
            .map(|text| text.title.capacity() + text.artist.capacity() + text.album.capacity())
            .sum::<usize>();
        let credits = playlist
            .tracks
            .iter()
            .map(|track| {
                track.artists.capacity() * size_of::<String>()
                    + track.artist_credits.capacity() * size_of::<ArtistCredit>()
            })
            .sum::<usize>();
        let cache_bytes = size_of::<Self>()
            + encoded_bytes
            + search_strings
            + credits
            + playlist.tracks.capacity() * size_of::<PlaylistTrack>()
            + tokens.capacity() * size_of::<SearchText>()
            + cover_candidates.capacity() * size_of::<String>()
            + cover_candidates.iter().map(String::capacity).sum::<usize>();
        Arc::new(Self {
            playlist,
            revision,
            tokens,
            duration,
            cover_candidates,
            has_audio,
            saved: Arc::new(Mutex::new(saved)),
            cache_bytes,
        })
    }

    pub(super) fn from_source(
        saved: SavedPlaylist,
        revision: Option<u64>,
        key: &PlaylistKey,
    ) -> Arc<Self> {
        Self::build(
            Arc::unwrap_or_clone(saved.playlist),
            revision,
            key,
            Some(saved.receipt),
            saved.encoded_bytes,
        )
    }

    pub(super) fn append_page(&mut self, mut page: Self) {
        self.playlist.tracks.append(&mut page.playlist.tracks);
        self.tokens.append(&mut page.tokens);
        self.duration = self
            .duration
            .zip(page.duration)
            .map(|(left, right)| left + right);
        super::artwork::add_cover_candidates(
            &mut self.cover_candidates,
            page.cover_candidates.iter().map(String::as_str),
        );
        self.has_audio |= page.has_audio;
    }

    pub(super) fn fresh(&self) -> bool {
        self.saved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|receipt| playlist_snapshot_fresh(receipt.saved_at_ms))
    }

    pub(super) fn receipt(&self) -> Option<PlaylistReceipt> {
        self.saved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(super) fn mark_saved(&self, receipt: PlaylistReceipt) {
        let mut saved = self.saved.lock().unwrap_or_else(PoisonError::into_inner);
        if saved.as_ref().is_none_or(|previous| {
            (receipt.generation(), receipt.saved_at_ms)
                > (previous.generation(), previous.saved_at_ms)
        }) {
            *saved = Some(receipt);
        }
    }

    fn generation(&self) -> i64 {
        self.saved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or(0, PlaylistReceipt::generation)
    }
}

#[derive(Default)]
pub(super) struct Cache {
    entries: VecDeque<(PlaylistKey, Arc<Snapshot>)>,
    revision: u64,
    bytes: usize,
    generation: i64,
}

impl Cache {
    pub(super) fn revision_for(&mut self, key: &PlaylistKey) -> Option<u64> {
        // Appending to a local playlist preserves the positions of its existing rows.
        let PlaylistKey::Source { .. } = key else {
            return None;
        };
        self.revision += 1;
        Some(self.revision)
    }

    pub(super) fn get(&mut self, key: &PlaylistKey) -> Option<Arc<Snapshot>> {
        let index = self.position(key)?;
        let entry = self.entries.remove(index)?;
        let snapshot = Arc::clone(&entry.1);
        self.entries.push_back(entry);
        Some(snapshot)
    }

    pub(super) fn remember(&mut self, key: PlaylistKey, snapshot: Arc<Snapshot>) {
        if !matches!(key, PlaylistKey::Source { .. }) {
            return;
        }
        if snapshot.generation() < self.generation {
            return;
        }
        self.forget(&key);
        if snapshot.cache_bytes > MAX_CACHED_BYTES {
            return;
        }
        self.bytes += snapshot.cache_bytes;
        self.entries.push_back((key, snapshot));
        while self.entries.len() > MAX_CACHED_PLAYLISTS || self.bytes > MAX_CACHED_BYTES {
            let (_, evicted) = self
                .entries
                .pop_front()
                .expect("a full cache has a snapshot");
            self.bytes -= evicted.cache_bytes;
        }
    }

    pub(super) fn forget(&mut self, key: &PlaylistKey) {
        if let Some(index) = self.position(key)
            && let Some((_, snapshot)) = self.entries.remove(index)
        {
            self.bytes -= snapshot.cache_bytes;
        }
    }

    pub(super) fn forget_account(&mut self, provider: ProviderId, profile: Option<&str>) {
        self.entries
            .retain(|(key, _)| !super::matches_playlist_account(key, provider, profile));
        self.bytes = self
            .entries
            .iter()
            .map(|(_, snapshot)| snapshot.cache_bytes)
            .sum();
    }

    fn position(&self, key: &PlaylistKey) -> Option<usize> {
        self.entries.iter().position(|(cached, _)| cached == key)
    }

    pub(super) fn retire(&mut self, generation: i64) {
        self.generation = self.generation.max(generation);
        self.entries
            .retain(|(_, snapshot)| snapshot.generation() >= self.generation);
        self.bytes = self
            .entries
            .iter()
            .map(|(_, snapshot)| snapshot.cache_bytes)
            .sum();
    }
}
