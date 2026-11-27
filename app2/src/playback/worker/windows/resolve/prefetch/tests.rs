use std::sync::{OnceLock, mpsc::Sender};

use crate::{playback::tests::entry, playlist::Media, providers::ProviderId};

use super::*;

mod native;

#[derive(Debug, Eq, PartialEq)]
pub(super) enum Event {
    Cancelled(usize),
    Settled { index: usize, ready: bool },
}

static EVENTS: OnceLock<Sender<Event>> = OnceLock::new();

pub(super) fn observe(event: Event) {
    if let Some(events) = EVENTS.get() {
        let _ = events.send(event);
    }
}

fn recording(index: usize) -> QueueEntry {
    entry(
        index,
        Some(Media::Provider {
            provider: ProviderId::YandexMusic,
            id: index.to_string(),
        }),
    )
}

#[test]
fn audio_prefetch_only_tracks_the_first_upcoming_occurrence() {
    let (worker, requests) = async_channel::bounded(1);
    let mut prefetch = Prefetch {
        worker,
        upcoming: None,
    };
    prefetch.replace(vec![recording(1), recording(2)], false);
    let initial = requests.try_recv().unwrap();
    assert_eq!(initial.entry.index, 1);

    prefetch.replace(vec![recording(3), recording(1)], false);
    let replacement = requests.try_recv().unwrap();
    assert!(initial.ticket.cancelled.load(Ordering::Acquire));
    assert_eq!(replacement.entry.index, 3);
    assert!(!replacement.ticket.cancelled.load(Ordering::Acquire));

    prefetch.replace(
        vec![
            entry(7, Some(Media::File("local.wav".into()))),
            recording(3),
        ],
        true,
    );
    assert!(replacement.ticket.cancelled.load(Ordering::Acquire));
    assert!(requests.is_empty());
}

#[test]
fn completed_attempts_are_eligible_for_one_renewal_in_the_final_window() {
    let (worker, requests) = async_channel::bounded(1);
    let mut prefetch = Prefetch {
        worker,
        upcoming: None,
    };
    prefetch.replace(vec![recording(1)], false);
    let initial = requests.try_recv().unwrap();
    initial.ticket.finished.store(true, Ordering::Release);

    prefetch.replace(vec![recording(1)], false);
    assert!(requests.is_empty());
    prefetch.replace(vec![recording(1)], true);
    assert_eq!(requests.try_recv().unwrap().entry.index, 1);
    assert!(initial.ticket.cancelled.load(Ordering::Acquire));
    prefetch.replace(vec![recording(1)], true);
    assert!(requests.is_empty());
}
