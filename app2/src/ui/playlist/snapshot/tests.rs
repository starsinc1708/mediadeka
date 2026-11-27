use super::*;
use crate::{playback::Status, playlist::PlaylistTrack, ui::player::Snapshot as PlaybackSnapshot};

fn key(profile: &str, id: usize) -> PlaylistKey {
    PlaylistKey::Source {
        provider: ProviderId::Jellyfin,
        profile: Some(profile.into()),
        id: id.to_string(),
    }
}

fn playlist(order: &[&str]) -> Playlist {
    Playlist {
        name: "Mix".into(),
        owner: None,
        artwork: None,
        link: None,
        tracks: order
            .iter()
            .map(|id| PlaylistTrack {
                id: (*id).into(),
                title: (*id).into(),
                artist: String::new(),
                artists: Vec::new(),
                artist_credits: Vec::new(),
                album_ref: None,
                album: None,
                artwork: None,
                added: None,
                duration: Some(30),
                media: None,
            })
            .collect(),
    }
}

#[test]
fn cache_keeps_recent_results_in_their_account_scope() {
    let mut cache = Cache::default();
    let snapshot = Snapshot::prepare(playlist(&["A"]), Some(1), &key("one", 0));
    for index in 0..MAX_CACHED_PLAYLISTS {
        cache.remember(key("one", index), Arc::clone(&snapshot));
    }
    assert!(Arc::ptr_eq(&cache.get(&key("one", 0)).unwrap(), &snapshot));
    assert!(cache.get(&key("two", 0)).is_none());
    cache.remember(key("two", 0), Arc::clone(&snapshot));
    assert!(cache.get(&key("one", 0)).is_some());
    assert!(cache.get(&key("one", 1)).is_none());
    cache.forget_account(ProviderId::Jellyfin, Some("one"));
    assert!(cache.get(&key("one", 0)).is_none());
    assert!(cache.get(&key("two", 0)).is_some());
    cache.forget_account(ProviderId::Jellyfin, None);
    assert!(cache.get(&key("two", 0)).is_none());
}

#[test]
fn large_rows_evict_by_retained_bytes_and_oversized_results_leave_other_entries() {
    let mut cache = Cache::default();
    let mut old = Snapshot::prepare(playlist(&["old"]), None, &key("one", 0));
    Arc::get_mut(&mut old).unwrap().cache_bytes = MAX_CACHED_BYTES / 2 + 1;
    let mut recent = Snapshot::prepare(playlist(&["recent"]), None, &key("one", 1));
    Arc::get_mut(&mut recent).unwrap().cache_bytes = MAX_CACHED_BYTES / 2 + 1;
    cache.remember(key("one", 0), old);
    cache.remember(key("one", 1), recent);
    assert!(cache.get(&key("one", 0)).is_none());
    let mut oversized = Snapshot::prepare(playlist(&["large"]), None, &key("one", 2));
    Arc::get_mut(&mut oversized).unwrap().cache_bytes = MAX_CACHED_BYTES + 1;
    cache.remember(key("one", 2), oversized);
    assert!(cache.get(&key("one", 1)).is_some());
    assert_eq!(cache.bytes, MAX_CACHED_BYTES / 2 + 1);
}

#[test]
fn clearing_playlist_memory_ignores_a_late_pre_clear_snapshot() {
    let mut cache = Cache::default();
    let key = key("one", 0);
    let old = Snapshot::prepare(playlist(&["Old"]), Some(1), &key);
    cache.remember(key.clone(), Arc::clone(&old));
    cache.retire(1);
    assert!(cache.get(&key).is_none());
    cache.remember(key.clone(), old);
    assert!(cache.get(&key).is_none());
}

#[test]
fn playback_highlights_one_occurrence_of_the_same_order() {
    let mut cache = Cache::default();
    let key = key("one", 0);
    let old = Snapshot::prepare(playlist(&["A", "B", "A"]), cache.revision_for(&key), &key);
    cache.remember(key.clone(), Arc::clone(&old));
    let playing = PlaybackSnapshot {
        key: Some(key.clone()),
        index: Some(2),
        playlist_revision: old.revision,
        status: Status::Playing,
        soloist_setup_required: false,
        queue_len: 3,
        queue_revision: 1,
        queue_position: Some(2),
        shuffle: false,
        continuation: None,
        sound_source: None,
        audio_quality: None,
    };
    assert!(!playing.highlights(&key, old.revision, 0));
    assert!(playing.highlights(&key, cache.get(&key).unwrap().revision, 2));

    let same_data = Snapshot::prepare(playlist(&["A", "B", "A"]), cache.revision_for(&key), &key);
    assert!(!playing.highlights(&key, same_data.revision, 2));
    let reordered = Snapshot::prepare(playlist(&["A", "A", "B"]), cache.revision_for(&key), &key);
    assert!(!playing.highlights(&key, reordered.revision, 1));
    assert!(!playing.highlights(&key, reordered.revision, 2));
}
