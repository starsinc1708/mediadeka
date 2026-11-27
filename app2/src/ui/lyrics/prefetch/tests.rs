use crate::playlist::{Media, PlaylistKey, PlaylistTrack};

use super::*;

#[test]
fn a_transient_failure_retries_without_a_queue_change_and_absence_does_not() {
    let mut prefetch = Prefetch::default();
    let now = Instant::now();
    let first = track(1);
    let second = track(2);
    prefetch.reconcile(None, [&first, &second].into_iter());
    prefetch.finish(first.clone(), Err("temporary".to_owned()), now);
    assert_eq!(prefetch.pending(now).unwrap().track.id, second.track.id);
    prefetch.finish(second, Ok(false), now);
    assert!(prefetch.pending(now).is_none());
    let first_retry = prefetch.next_retry().unwrap();
    assert_eq!(first_retry, now + RETRY_DELAY);
    assert_eq!(
        prefetch.pending(first_retry).unwrap().track.id,
        first.track.id
    );
    prefetch.finish(first, Ok(true), first_retry);
    assert!(prefetch.next_retry().is_none());
    assert!(prefetch.pending(first_retry + RETRY_MAX_DELAY).is_none());
}

#[test]
fn retry_backoff_is_bounded_and_discarded_when_the_recording_leaves_the_window() {
    let mut prefetch = Prefetch::default();
    let track = track(1);
    prefetch.reconcile(None, [&track].into_iter());
    let mut now = Instant::now();
    for delay in [20, 40, 80, 120, 120] {
        prefetch.finish(track.clone(), Err("temporary".to_owned()), now);
        let expected = now + Duration::from_secs(delay);
        assert_eq!(prefetch.next_retry(), Some(expected));
        assert!(
            prefetch
                .pending(expected - Duration::from_millis(1))
                .is_none()
        );
        assert!(prefetch.pending(expected).is_some());
        now = expected;
    }
    prefetch.reconcile(None, std::iter::empty());
    assert!(prefetch.next_retry().is_none());
    assert!(prefetch.pending(now).is_none());
}

fn track(index: usize) -> QueueEntry {
    let id = index.to_string();
    QueueEntry {
        playlist: PlaylistKey::Local(1),
        playlist_revision: None,
        index,
        track: PlaylistTrack {
            id: id.clone(),
            title: format!("Трек {id}"),
            artist: String::new(),
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: None,
            artwork: None,
            added: None,
            duration: None,
            media: Some(Media::File(format!("{id}.mp3").into())),
        },
        recommendations: Vec::new(),
    }
}
