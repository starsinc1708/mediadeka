#[cfg(windows)]
pub(super) mod windows;

use crate::playlist::{Media, PlaylistKey, PlaylistTrack};
use crate::providers::ProviderId;

use super::queue::Placement;
use super::{Playback, QueueEntry, RecommendationOrigin, Repeat, SoundSource, Status};

pub(super) fn entry(index: usize, media: Option<Media>) -> QueueEntry {
    QueueEntry {
        playlist_revision: None,
        recommendations: Vec::new(),
        playlist: PlaylistKey::Local(1),
        index,
        track: PlaylistTrack {
            id: "same-recording".into(),
            title: format!("Recording {index}"),
            artist: "Test artist".into(),
            artwork: None,
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: None,
            added: None,
            duration: None,
            media,
        },
    }
}

#[test]
fn station_feedback_keeps_the_skipped_entry_and_refill_continues_at_eof() {
    let make = |index| {
        let mut entry = entry(index, Some(Media::File("audio.mp3".into())));
        entry.playlist = PlaylistKey::Station {
            provider: ProviderId::YandexMusic,
            profile: Some("profile".into()),
            id: "user:onyourwave".into(),
        };
        entry.playlist_revision = Some(1);
        entry
    };
    let mut playback = Playback::new();
    playback.queue.replace(vec![make(0), make(1)], 0);
    playback.frame.status = Status::Playing;
    playback.frame.source = Some(SoundSource::LocalFile);
    playback.frame.position = 4.0;
    playback.report_start();
    playback.next();
    let skipped = playback.take_station_ended().expect("skip after sound");
    assert_eq!(skipped.entry.index, 0);
    assert_eq!(skipped.seconds, 4.0);
    assert!(skipped.skipped);
    assert_eq!(playback.current().unwrap().index, 1);

    playback.frame.status = Status::Playing;
    playback.frame.source = Some(SoundSource::LocalFile);
    playback.report_start();
    playback.frame.completed = true;
    playback.append_continuation(vec![make(2)]);
    assert_eq!(playback.current().unwrap().index, 2);
    assert!(playback.take_station_ended().is_none());
}

#[test]
fn playlist_selection_and_queue_commands_keep_distinct_occurrences() {
    let make = |index| entry(index, Some(Media::File("audio.mp3".into())));
    let mut queue = super::Queue::default();
    queue.replace(vec![make(0), entry(1, None), make(2), make(3)], 2);
    assert_eq!(queue.current_index(), Some(1));
    assert_eq!(queue.current().unwrap().index, 2);
    queue.insert(vec![make(7), entry(8, None), make(9)], Placement::Next);
    queue.insert(vec![make(8)], Placement::ManualEnd);
    let order = |queue: &super::Queue| queue.entries().map(|entry| entry.index).collect::<Vec<_>>();
    assert_eq!(order(&queue), [0, 2, 7, 9, 3, 8]);
    queue.toggle_shuffle();
    assert_eq!(order(&queue)[..2], [0, 2]);
    assert_eq!(queue.current().unwrap().index, 2);
    queue.toggle_shuffle();
    assert_eq!(order(&queue), [0, 2, 7, 9, 3, 8]);
    assert!(queue.select(5));
    assert_eq!(queue.next(false), None);
    assert_eq!(queue.next(true), Some(0));
    assert_eq!(queue.previous(false), Some(4));

    let mut playback = Playback::new();
    playback.play(vec![entry(0, None)], 0);
    assert_eq!(playback.status(), &Status::Idle);
    assert!(playback.error().unwrap().contains("недоступен"));
    assert!(!playback.needs_poll());
    assert!(playback.repeat() == Repeat::Off);
    playback.cycle_repeat();
    assert!(playback.repeat() == Repeat::All);
    playback.cycle_repeat();
    assert!(playback.repeat() == Repeat::One);
}

#[test]
fn reordered_queue_preserves_occurrence_identity_and_the_playing_cursor() {
    let make = |index| entry(index, Some(Media::File("audio.mp3".into())));
    let mut queue = super::Queue::default();
    queue.replace(vec![make(0), make(1), make(1), make(2)], 0);
    let revision = queue.revision();
    let ids: Vec<_> = (0..4).map(|index| queue.at(index).unwrap().0).collect();
    assert!(!queue.move_upcoming(revision, ids[0], 0, 1));
    assert!(queue.move_upcoming(revision, ids[1], 1, 3));
    assert!(!queue.move_upcoming(revision, ids[2], 1, 2));
    assert_eq!(queue.current().unwrap().index, 0);
    assert_eq!(queue.current_index(), Some(0));
    assert_eq!(queue.replacement_revision(), revision);
    let ordered = [ids[0], ids[2], ids[3], ids[1]];
    let current_ids =
        |queue: &super::Queue| std::array::from_fn::<_, 4, _>(|index| queue.at(index).unwrap().0);
    assert_eq!(current_ids(&queue), ordered);
    queue.toggle_shuffle();
    queue.toggle_shuffle();
    assert_eq!(current_ids(&queue), ordered);
}

#[test]
fn removing_an_upcoming_occurrence_preserves_the_cursor_and_the_other_duplicate() {
    let make = |index| entry(index, Some(Media::File("audio.mp3".into())));
    let mut queue = super::Queue::default();
    queue.replace(vec![make(0), make(1), make(2), make(2)], 1);
    let ids: Vec<_> = (0..4).map(|index| queue.at(index).unwrap().0).collect();
    let revision = queue.revision();
    assert!(!queue.remove_upcoming(revision, ids[0], 0));
    assert!(!queue.remove_upcoming(revision, ids[1], 1));
    assert!(queue.remove_upcoming(revision, ids[2], 2));
    assert!(!queue.remove_upcoming(revision, ids[3], 2));
    assert_eq!(queue.replacement_revision(), revision);
    assert_eq!(queue.current_index(), Some(1));
    assert_eq!(queue.current().unwrap().index, 1);
    assert_eq!(queue.at(2).unwrap().0, ids[3]);
    queue.toggle_shuffle();
    queue.toggle_shuffle();
    assert_eq!(queue.at(2).unwrap().0, ids[3]);
    assert_eq!(queue.entries().len(), 3);
}

#[test]
fn clearing_upcoming_keeps_the_shuffled_playback_prefix_and_playhead() {
    use super::queue_resume::{QueueResumePlan, ResumeEntry};

    let mut playback = Playback::new();
    let entries = (0..5)
        .map(|index| ResumeEntry {
            entry: entry(index, Some(Media::File("audio.mp3".into()))),
            autoplay: false,
        })
        .collect();
    let order = vec![4, 2, 0, 3, 1];
    playback
        .queue
        .restore(QueueResumePlan::new(entries, order, true, 0, 0), Some(1));
    playback.frame.status = Status::Playing;
    playback.frame.source = Some(SoundSource::LocalFile);
    playback.frame.position = 12.5;
    let revision = playback.queue_revision();
    let retained_ids = [0, 1].map(|position| playback.queued(position).unwrap().0);

    assert!(!playback.clear_upcoming(revision.wrapping_sub(1)));
    assert!(playback.has_queued_next());
    assert!(playback.clear_upcoming(revision));
    assert!(!playback.has_queued_next());
    assert_eq!(
        playback
            .entries()
            .map(|entry| entry.index)
            .collect::<Vec<_>>(),
        [4, 2]
    );
    assert_eq!(
        [0, 1].map(|position| playback.queued(position).unwrap().0),
        retained_ids
    );
    assert_eq!(playback.current_index(), Some(1));
    assert_eq!(playback.current().unwrap().index, 2);
    assert_eq!(playback.position(), 12.5);
    assert_eq!(playback.sound_source(), Some(&SoundSource::LocalFile));
    assert_eq!(playback.status(), &Status::Playing);
    assert_eq!(playback.queue_replacement_revision(), revision);
    assert_eq!(playback.queue_revision(), revision.wrapping_add(1));
    assert!(!playback.clear_upcoming(playback.queue_revision()));
}

#[test]
fn mix_origins_follow_the_recording_through_queue_changes() {
    let make = |index| entry(index, Some(Media::File("audio.mp3".into())));
    let mut queue = super::Queue::default();
    queue.replace(vec![make(0), make(1)], 0);
    let previous_revision = queue.revision();
    let mut recommended = make(2);
    recommended.recommendations = vec![
        RecommendationOrigin {
            provider: ProviderId::Spotify,
            playlist_id: "mix-spotify".into(),
            playlist: "Daily Mix".into(),
        },
        RecommendationOrigin {
            provider: ProviderId::YandexMusic,
            playlist_id: "mix-yandex".into(),
            playlist: "Плейлист дня".into(),
        },
    ];
    queue.replace(vec![make(0), recommended.clone()], 0);
    assert_ne!(queue.revision(), previous_revision);
    queue.insert(vec![make(3)], Placement::Next);
    queue.toggle_shuffle();

    let mixed = queue.entries().position(|entry| entry.index == 2).unwrap();
    assert!(queue.select(mixed));
    assert_eq!(
        queue.current().unwrap().recommendations,
        recommended.recommendations
    );
    assert!(
        queue
            .entries()
            .filter(|entry| entry.index != 2)
            .all(|entry| entry.recommendations.is_empty())
    );
}

#[test]
fn manual_additions_keep_priority_over_autoplay_with_and_without_shuffle() {
    let make = |index| entry(index, Some(Media::File("audio.mp3".into())));
    let mut queue = super::Queue::default();
    queue.replace(vec![make(0)], 0);
    queue.insert(vec![make(1), make(2)], Placement::AutoplayEnd);
    assert!(queue.select(1));
    queue.insert(vec![make(3)], Placement::ManualEnd);
    queue.insert(vec![make(4)], Placement::Next);
    let order = |queue: &super::Queue| queue.entries().map(|entry| entry.index).collect::<Vec<_>>();
    assert_eq!(order(&queue), [0, 1, 4, 3, 2]);
    queue.toggle_shuffle();
    assert_eq!(order(&queue)[..2], [0, 1]);
    assert_eq!(order(&queue)[4], 2);
    queue.toggle_shuffle();
    assert_eq!(order(&queue), [0, 1, 4, 3, 2]);
}

#[test]
fn stale_completion_cannot_change_a_replacement_or_override_repeat() {
    let make = |index| entry(index, Some(Media::File("audio.mp3".into())));
    let mut playback = Playback::new();
    playback.queue.replace(vec![make(0)], 0);
    assert!(playback.continuation_point().is_none());
    playback.frame.completed = true;
    let point = playback.continuation_point().unwrap();
    playback.cycle_repeat();
    playback.continue_after(point, vec![make(1)]);
    assert_eq!(playback.entries().len(), 1);
    playback.cycle_repeat();
    assert!(playback.continuation_point().is_none());
    playback.cycle_repeat();
    playback.queue.replace(vec![make(2)], 0);
    playback.continue_after(point, vec![make(1)]);
    assert_eq!(playback.entries().len(), 1);
    assert_eq!(playback.current().unwrap().index, 2);
}

#[test]
fn manual_entry_resumes_a_finished_queue_and_invalidates_the_pending_seed() {
    let make = |index| entry(index, Some(Media::File("audio.mp3".into())));
    let mut playback = Playback::new();
    playback.queue.replace(vec![make(0)], 0);
    playback.frame.completed = true;
    let point = playback.continuation_point().unwrap();
    playback.append(vec![make(1)]);
    assert!(playback.has_queued_next());
    playback.continue_after(point, Vec::new());
    assert_eq!(playback.current().unwrap().index, 1);
    playback.continue_after(point, vec![make(2)]);
    assert_eq!(playback.entries().len(), 2);
}

#[test]
fn playlist_shuffle_starts_with_an_available_occurrence() {
    let make = |index| entry(index, Some(Media::File("audio.mp3".into())));
    let mut playback = Playback::new();
    playback.play_shuffled(vec![entry(0, None), make(1), make(2), make(3)]);

    assert!(playback.shuffle());
    assert_eq!(playback.current_index(), Some(0));
    let current = playback.current().unwrap().index;
    assert_eq!(playback.entries().next().unwrap().index, current);
    let mut indices = playback
        .entries()
        .map(|entry| entry.index)
        .collect::<Vec<_>>();
    indices.sort_unstable();
    assert_eq!(indices, [1, 2, 3]);

    playback.play_shuffled(vec![entry(0, None)]);
    assert_eq!(playback.current().unwrap().index, current);
    assert_eq!(playback.error(), Some(super::UNAVAILABLE_TRACK));

    playback.play(vec![make(1), make(2), make(3)], 1);
    assert!(playback.shuffle());
    assert_eq!(playback.current_index(), Some(0));
    assert_eq!(playback.current().unwrap().index, 2);
}

#[test]
fn one_sound_message_per_generation() {
    let mut playback = Playback::new();
    playback
        .queue
        .replace(vec![entry(0, Some(Media::File("audio.mp3".into())))], 0);
    playback.generation = 1;
    // The one-frame channel may have replaced the playing frame with a pause.
    playback.frame.status = Status::Paused;
    playback.frame.source = Some(SoundSource::LocalFile);
    playback.report_start();
    let started = playback.take_sound_started().expect("first sound");
    assert_eq!(started.entry.track.title, "Recording 0");
    assert_eq!(started.source, SoundSource::LocalFile);
    assert!(playback.take_sound_started().is_none());

    // A repeated frame of the same generation is not news.
    playback.report_start();
    assert!(playback.take_sound_started().is_none());

    // Pause, resume and seeking all keep the generation.
    playback.frame.status = Status::Paused;
    playback.report_start();
    playback.frame.status = Status::Playing;
    playback.frame.position = 12.5;
    playback.report_start();
    playback.report_start();
    assert!(playback.take_sound_started().is_none());

    // A new generation reports again.
    playback.start_current(0.);
    playback.frame.status = Status::Playing;
    playback.frame.source = Some(SoundSource::LocalFile);
    playback.report_start();
    assert!(
        playback.take_sound_started().is_some(),
        "a new generation reports again"
    );
    playback.report_start();
    assert!(playback.take_sound_started().is_none());
}

#[test]
fn replaying_the_same_entry_starts_a_new_generation() {
    let mut playback = Playback::new();
    let media = Some(Media::File("audio.mp3".into()));
    playback.play(vec![entry(0, media.clone()), entry(1, media)], 0);
    let first = playback.generation;
    playback.frame.status = Status::Playing;
    playback.frame.source = Some(SoundSource::LocalFile);
    playback.report_start();
    assert!(playback.take_sound_started().is_some());

    // The same record launched again is a new generation with a new message.
    playback.play(
        vec![
            entry(0, Some(Media::File("audio.mp3".into()))),
            entry(1, Some(Media::File("audio.mp3".into()))),
        ],
        0,
    );
    assert_ne!(playback.generation, first);
    playback.frame.status = Status::Playing;
    playback.frame.source = Some(SoundSource::LocalFile);
    playback.report_start();
    let again = playback.take_sound_started().expect("replayed sound");
    assert_eq!(again.entry.index, playback.current().unwrap().index);
}

#[test]
fn listening_event_uses_the_playing_source_instead_of_the_catalog_source() {
    let mut playback = Playback::new();
    let entry = entry(
        0,
        Some(Media::Provider {
            provider: ProviderId::Spotify,
            id: "track".into(),
        }),
    );
    playback.queue.replace(vec![entry.clone()], 0);
    playback.frame.status = Status::Playing;
    playback.frame.source = Some(SoundSource::Stream(ProviderId::YandexMusic));
    playback.report_start();
    let started = playback.take_sound_started().unwrap();
    assert_eq!(started.entry.track, entry.track);
    assert_eq!(started.source, SoundSource::Stream(ProviderId::YandexMusic));
}
