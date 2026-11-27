use super::super::Playback;
use super::{Frame, Status, WORKER_STOPPED, Worker, mpsc};

fn playback_with_disconnected_worker() -> Playback {
    let (commands, _requests) = mpsc::channel();
    let (_output, frames) = async_channel::bounded(1);
    Playback {
        worker: Some(Worker {
            commands,
            frames,
            #[cfg(windows)]
            signals: Default::default(),
            #[cfg(windows)]
            stopped: mpsc::channel().1,
        }),
        frame: Frame {
            status: Status::Loading,
            volume: 0.35,
            ..Frame::default()
        },
        sequence: 1,
        ..Playback::default()
    }
}

fn assert_recoverable_failure(playback: &Playback) {
    assert_eq!(playback.status(), &Status::Failed(WORKER_STOPPED.into()));
    assert!(playback.worker.is_none());
    assert!(!playback.needs_poll());
    assert!(!playback.soloist_setup_required());
    assert!((playback.volume() - 0.35).abs() < 0.001);
}

#[test]
fn closed_command_channel_finishes_pending_load() {
    let mut playback = playback_with_disconnected_worker();
    playback.set_volume(0.5);
    assert_recoverable_failure(&playback);
}

#[test]
fn closed_event_channel_finishes_pending_load() {
    let mut playback = playback_with_disconnected_worker();
    assert_eq!(playback.poll(), super::PollChange::Transport);
    assert_recoverable_failure(&playback);
}

#[test]
fn steady_position_and_background_acknowledgements_do_not_change_transport() {
    use super::PollChange;

    let frame = Frame {
        status: Status::Playing,
        ..Frame::default()
    };
    let mut next = frame.clone();
    next.position = 2.5;
    next.sequence += 1;
    next.cache_revision += 1;
    assert_eq!(frame.change(&next), PollChange::Position);

    next.position = frame.position;
    assert_eq!(frame.change(&next), PollChange::None);

    next.status = Status::Paused;
    assert_eq!(frame.change(&next), PollChange::Transport);
}

#[test]
fn setup_retry_loads_the_same_queue_occurrence_at_its_last_position() {
    use super::{Command, Message, QueueEntry};
    use crate::playlist::{Media, PlaylistKey, PlaylistTrack};
    use crate::providers::ProviderId;

    let mut playback = playback_with_disconnected_worker();
    let (commands, requests) = mpsc::channel();
    playback.worker.as_mut().unwrap().commands = commands;
    let track_id = "0On1SxfkvTOfzvjVgFLK7V";
    let entry = QueueEntry {
        playlist: PlaylistKey::Source {
            provider: ProviderId::Spotify,
            profile: Some("playlist-account".into()),
            id: "playlist".into(),
        },
        playlist_revision: None,
        recommendations: Vec::new(),
        index: 3,
        track: PlaylistTrack {
            id: track_id.into(),
            title: "Track".into(),
            artist: String::new(),
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: None,
            artwork: None,
            added: None,
            duration: Some(80),
            media: Some(Media::Provider {
                provider: ProviderId::Spotify,
                id: track_id.into(),
            }),
        },
    };
    playback.queue.replace(vec![entry], 0);
    playback.frame.status = Status::Failed("Нужно сопряжение.".into());
    playback.frame.position = 42.5;
    playback.frame.soloist_setup_required = true;

    playback.toggle_pause();
    assert_eq!(playback.status(), &Status::Loading);
    assert!(!playback.soloist_setup_required());
    assert_eq!(playback.position(), 42.5);
    assert!(matches!(requests.try_recv(), Ok(Message::Request {
        command: Command::Load { entry, position: 42.5, .. }, ..
    }) if entry.index == 3 && entry.track.id == track_id));
}
