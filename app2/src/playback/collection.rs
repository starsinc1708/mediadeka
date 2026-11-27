use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::UNIX_EPOCH,
};

use serde::{Deserialize, Serialize};

use crate::playlist::{PlaylistKey, PlaylistTrack};

use super::QueueEntry;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CollectionKey {
    Playlist(PlaylistKey),
    LocalAlbum { title: String, artist: String },
}

impl CollectionKey {
    pub(crate) fn connected_source(&self) -> Option<(crate::providers::ProviderId, Option<&str>)> {
        match self {
            Self::Playlist(key) => key.connected_source(),
            Self::LocalAlbum { .. } => None,
        }
    }

    pub(crate) fn is_album(&self) -> bool {
        match self {
            Self::LocalAlbum { .. } => true,
            Self::Playlist(PlaylistKey::Source { id, .. }) => id.starts_with("album:"),
            _ => false,
        }
    }
}

/// Ordered playable occurrences in the original album or playlist. Indices
/// refer to occurrences, so repeated copies of a track count independently.
pub(crate) struct CollectionPlan {
    pub session: String,
    pub key: CollectionKey,
    pub title: String,
    pub artwork: Option<String>,
    pub entries: Vec<QueueEntry>,
}

impl CollectionPlan {
    pub(crate) fn new<'a>(
        key: PlaylistKey,
        title: &str,
        artwork: Option<&str>,
        tracks: impl IntoIterator<Item = &'a PlaylistTrack>,
    ) -> Self {
        Self::from_entries(
            CollectionKey::Playlist(key.clone()),
            title.to_owned(),
            artwork.map(str::to_owned),
            tracks
                .into_iter()
                .enumerate()
                .filter(|(_, track)| track.media.is_some())
                .map(|(index, track)| QueueEntry {
                    playlist: key.clone(),
                    playlist_revision: None,
                    recommendations: Vec::new(),
                    index,
                    track: track.clone(),
                })
                .collect(),
        )
    }

    pub(crate) fn from_entries(
        key: CollectionKey,
        title: String,
        artwork: Option<String>,
        entries: Vec<QueueEntry>,
    ) -> Self {
        static SESSION_SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let ticks = UNIX_EPOCH.elapsed().unwrap_or_default().as_nanos();
        let sequence = SESSION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let process = std::process::id();
        Self {
            session: format!("{ticks}-{process}-{sequence}"),
            key,
            title,
            artwork,
            entries,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct CollectionProgress {
    pub index: usize,
    pub position_ms: u64,
    duration_ms: u64,
    total: usize,
    completed: BTreeSet<usize>,
    #[serde(default)]
    pub at_end: bool,
}

impl CollectionProgress {
    pub(crate) fn fraction(&self) -> f32 {
        let current = if !self.completed.contains(&self.index) && self.duration_ms > 0 {
            (self.position_ms as f32 / self.duration_ms as f32).min(0.99)
        } else {
            0.
        };
        (self.completed.len() as f32 + current) / self.total.max(1) as f32
    }

    pub(crate) fn resumable(&self) -> bool {
        self.completed.len() < self.total
    }

    pub(crate) fn completed(&self, index: usize) -> bool {
        self.completed.contains(&index)
    }

    pub(crate) fn resume_index(&self, plan: &CollectionPlan) -> Option<usize> {
        let position = plan
            .entries
            .iter()
            .position(|entry| entry.index == self.index)?;
        if !self.at_end {
            return Some(position);
        }
        plan.entries
            .iter()
            .enumerate()
            .cycle()
            .skip(position)
            .take(plan.entries.len())
            .find(|(_, entry)| !self.completed(entry.index))
            .map(|(position, _)| position)
    }
}

#[derive(Clone)]
pub(crate) struct CollectionProgressUpdate {
    pub plan: Arc<CollectionPlan>,
    pub progress: CollectionProgress,
}

pub(super) struct CollectionListening {
    plan: Arc<CollectionPlan>,
    progress: CollectionProgress,
}

impl CollectionListening {
    pub(super) fn new(plan: Arc<CollectionPlan>, mut progress: CollectionProgress) -> Self {
        progress.total = plan.entries.len();
        Self { plan, progress }
    }

    pub(super) fn observe(
        &mut self,
        entry: &QueueEntry,
        position: f64,
        duration: Option<f64>,
        at_end: bool,
        force: bool,
    ) -> Option<CollectionProgressUpdate> {
        let slot = self
            .plan
            .entries
            .binary_search_by_key(&entry.index, |entry| entry.index)
            .ok()?;
        let planned = &self.plan.entries[slot];
        if planned.playlist != entry.playlist || planned.track.id != entry.track.id {
            return None;
        }
        let position_ms = (position * 1_000.) as u64;
        // Persist changes on track, pause and EOF, and every five seconds
        // during playback. Ordinary position frames stay in the player.
        const CHECKPOINT_INTERVAL_MS: u64 = 5_000;
        let changed_bucket = position_ms / CHECKPOINT_INTERVAL_MS
            != self.progress.position_ms / CHECKPOINT_INTERVAL_MS;
        if !force && !changed_bucket && self.progress.index == entry.index && !at_end {
            return None;
        }
        self.progress.index = entry.index;
        self.progress.duration_ms = (duration.unwrap_or_default() * 1_000.) as u64;
        self.progress.position_ms = position_ms;
        self.progress.at_end = at_end;
        if at_end {
            self.progress.completed.insert(entry.index);
        }
        Some(CollectionProgressUpdate {
            plan: self.plan.clone(),
            progress: self.progress.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{playback::SoundSource, playlist::Media, storage::Store};

    use super::*;

    #[test]
    fn counts_eof_and_current_position_without_completing_skipped_occurrences() {
        let track = PlaylistTrack {
            id: "repeated".into(),
            title: "Track".into(),
            artist: String::new(),
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: None,
            artwork: None,
            added: None,
            duration: Some(10),
            media: Some(Media::File("recording.wav".into())),
        };
        let plan = Arc::new(CollectionPlan::new(
            PlaylistKey::Local(1),
            "Playlist",
            None,
            [&track; 3],
        ));
        let mut listening = CollectionListening::new(plan.clone(), CollectionProgress::default());
        listening
            .observe(&plan.entries[0], 4., Some(10.), false, true)
            .unwrap();
        let last = listening
            .observe(&plan.entries[1], 10., Some(10.), true, false)
            .unwrap();
        assert!(last.progress.completed(1) && !last.progress.completed(0));
        assert!((last.progress.fraction() - 1. / 3.).abs() < 0.01);
        assert_eq!(last.progress.resume_index(&plan), Some(2));
        let paused = listening
            .observe(&plan.entries[2], 5., Some(10.), false, true)
            .unwrap();
        assert_eq!(paused.progress.fraction(), 0.5);
        assert_eq!(paused.progress.position_ms, 5_000);
        assert_eq!(paused.progress.resume_index(&plan), Some(2));
        let ended = listening
            .observe(&plan.entries[2], 10., Some(10.), true, false)
            .unwrap();
        assert_eq!(ended.progress.resume_index(&plan), Some(0));
    }

    #[test]
    fn restores_local_album_queue_without_changing_track_history_origins() {
        let profile = tempfile::tempdir().unwrap();
        let store = Store::open(profile.path()).unwrap();
        let album = CollectionKey::LocalAlbum {
            title: "Live".into(),
            artist: "Artist".into(),
        };
        let entries = [7, 10]
            .into_iter()
            .enumerate()
            .map(|(index, id)| QueueEntry {
                playlist: PlaylistKey::Local(id),
                playlist_revision: None,
                recommendations: Vec::new(),
                index,
                track: PlaylistTrack {
                    id: format!("track-{index}"),
                    title: format!("Track {index}"),
                    artist: "Artist".into(),
                    artists: Vec::new(),
                    artist_credits: Vec::new(),
                    album_ref: None,
                    album: Some("Live".into()),
                    artwork: None,
                    added: None,
                    duration: Some(10),
                    media: Some(Media::File(format!("track-{index}.wav").into())),
                },
            })
            .collect();
        let plan = Arc::new(CollectionPlan::from_entries(
            album.clone(),
            "Live".into(),
            None,
            entries,
        ));
        let mut listening = CollectionListening::new(plan.clone(), Default::default());
        listening
            .observe(&plan.entries[0], 10., Some(10.), true, false)
            .unwrap();
        let update = listening
            .observe(&plan.entries[1], 2.5, Some(10.), false, true)
            .unwrap();
        store.save_collection_listening(&update, true).unwrap();
        let recorded = &plan.entries[1];
        store
            .record_history(&recorded.playlist, &recorded.track, SoundSource::LocalFile)
            .unwrap();
        drop(store);

        let store = Store::open(profile.path()).unwrap();
        let records = store.collection_records().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].key, album);
        assert!(records[0].key.is_album() && records[0].progress.resumable());
        let (loaded, progress) = store.collection_listening(album).unwrap().unwrap();
        assert_eq!(progress.resume_index(&loaded), Some(1));
        assert_eq!(progress.position_ms, 2_500);
        assert_eq!(loaded.entries[0].playlist, PlaylistKey::Local(7));
        assert_eq!(loaded.entries[1].playlist, PlaylistKey::Local(10));
        assert_eq!(
            store.listening_history().unwrap()[0].key,
            PlaylistKey::Local(10)
        );
    }
}
