use std::{
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use crate::{
    playback::{Playback, Status, quality::validate, tests::windows::Loopback},
    playlist::{PlaylistKey, PlaylistTrack},
};

use super::*;

fn entry() -> QueueEntry {
    QueueEntry {
        playlist: PlaylistKey::Source {
            provider: ProviderId::Spotify,
            id: "fixture-playlist".into(),
            profile: Some("unconnected".into()),
        },
        playlist_revision: None,
        recommendations: Vec::new(),
        index: 0,
        track: PlaylistTrack {
            id: "0000000000000000000001".into(),
            title: "Cached recording".into(),
            artist: "Fixture".into(),
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: None,
            artwork: None,
            added: None,
            duration: Some(3),
            media: Some(Media::Provider {
                provider: ProviderId::Spotify,
                id: "0000000000000000000001".into(),
            }),
        },
    }
}

#[test]
fn cancellation_stops_the_decision_before_provider_work() {
    let request = Resolve {
        generation: 1,
        entry: entry(),
        cancelled: Arc::new(AtomicBool::new(true)),
        rejected: Vec::new(),
        setup_failed: false,
        prefetched: None,
    };
    assert!(
        matches!(resolve(&request), Err(Failure::Track(message)) if message.contains("отменён"))
    );
}

#[test]
#[ignore = "requires libmpv, WASAPI and a 3-second MEDIADEKA_AUDIO_CACHE_FIXTURE"]
fn windows_stage5_profile_cache_reopens_offline_and_drives_the_queue() {
    const CHILD: &str = "MEDIADEKA_STAGE5_OFFLINE_CHILD";
    let fixture = PathBuf::from(std::env::var_os("MEDIADEKA_AUDIO_CACHE_FIXTURE").unwrap());
    let entry = entry();
    if std::env::var_os(CHILD).is_none() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let quality = validate(&fixture, entry.track.duration, &AtomicBool::new(false)).unwrap();
        assert!(
            store
                .publish_cached_audio(&entry, &fixture, &quality)
                .unwrap()
        );
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "windows_stage5_profile_cache_reopens_offline_and_drives_the_queue",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("MEDIADEKA_PROFILE_DIR", directory.path())
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&child.stdout));
        eprint!("{}", String::from_utf8_lossy(&child.stderr));
        assert!(child.status.success());
        return;
    }
    let store = Store::open_default().unwrap();
    let cached = store.cached_audio(&entry).unwrap().unwrap();
    assert_eq!(store.cached_recordings().unwrap().len(), 1);
    let mut direct = entry.clone();
    direct.track.media = Some(Media::File(fixture));
    let meter = Loopback::start();
    let mut playback = Playback::new();
    playback.set_volume(0.3);
    playback.play(vec![direct, entry.clone()], 0);
    wait(&mut playback, |playback| {
        playback.sound_source() == Some(&SoundSource::LocalFile)
    });
    let first = meter.wait(|peak| peak > 0.0001);
    playback.next();
    wait(&mut playback, |playback| {
        playback.sound_source() == Some(&SoundSource::Cached) && playback.position() > 0.1
    });
    let started = playback.take_sound_started().unwrap();
    store.record_sound(&started).unwrap();
    assert_eq!(
        playback.audio_quality().unwrap().codec,
        cached.quality.codec
    );
    assert_eq!(
        playback.audio_quality().unwrap().sample_rate_hz,
        cached.quality.sample_rate_hz
    );
    let restored = meter.wait(|peak| peak > 0.0001);
    let history = store.listening_history().unwrap().remove(0);
    assert_eq!(history.source, SoundSource::Cached);
    assert_eq!(history.details, started.details);
    assert!(history.details.time_to_sound_ms.is_some());
    println!(
        "WASAPI local_peak={first:.6}, cache_peak={restored:.6}, position={:.3}s, quality={}, startup={:?} ms",
        playback.position(),
        cached.quality.label(),
        history.details.time_to_sound_ms
    );
    let mut missing = entry;
    missing.track.id = "0000000000000000000002".into();
    missing.track.media = Some(Media::Provider {
        provider: ProviderId::Spotify,
        id: missing.track.id.clone(),
    });
    playback.play(vec![missing], 0);
    wait(
        &mut playback,
        |playback| matches!(playback.status(), Status::Failed(message) if message.contains("В кэше нет готовой копии")),
    );
    assert!(playback.sound_source().is_none());
}

fn wait(playback: &mut Playback, ready: impl Fn(&Playback) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(7);
    while !ready(playback) {
        playback.poll();
        assert!(
            Instant::now() < deadline,
            "playback: {:?}",
            playback.status()
        );
        thread::sleep(Duration::from_millis(20));
    }
}
