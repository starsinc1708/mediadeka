use super::{
    Duration, Instant, Loopback, Playback, ProviderId, QueueEntry, Status, Store, wait_until,
};
use std::sync::atomic::AtomicBool;

use crate::{
    music::{ObjectKind, ObjectRef},
    playback::SoundSource,
    playlist::{Media, PlaylistKey},
    providers::{self, catalog},
};

const START_DEADLINE: Duration = Duration::from_secs(35);
const AUDIBLE: f32 = 0.0001;

#[test]
#[ignore = "requires an isolated profile with paired Spotify Soloist, SoundCloud, subscribed Yandex Music and WASAPI"]
fn windows_cross_provider_next_and_natural_transition() {
    assert!(
        std::env::var_os("MEDIADEKA_PROFILE_DIR").is_some(),
        "use an isolated test profile with paired Soloist"
    );
    let store = Store::open_default().unwrap();
    let soundcloud = recording(
        &store,
        ProviderId::SoundCloud,
        "MEDIADEKA_TRANSITION_SOUNDCLOUD_PLAYLIST",
        "1841554947",
    );
    let yandex = recording(
        &store,
        ProviderId::YandexMusic,
        "MEDIADEKA_TRANSITION_YANDEX",
        "10741527:1612097",
    );
    let spotify = std::env::var_os("MEDIADEKA_TRANSITION_OMIT_SOLOIST")
        .is_none()
        .then(|| {
            recording(
                &store,
                ProviderId::Spotify,
                "MEDIADEKA_TRANSITION_SPOTIFY",
                "2Fxmhks0bxGSBdJ92vM42m",
            )
        });
    let meter = Loopback::start();
    let mut playback = Playback::new();
    playback.set_volume(0.25);
    wait_until(&mut playback, START_DEADLINE, |p| {
        (p.volume() - 0.25).abs() < 0.001
    });

    let mut transitions = vec![(&soundcloud, &yandex), (&yandex, &soundcloud)];
    if let Some(spotify) = &spotify {
        transitions.extend([(spotify, &soundcloud), (&soundcloud, spotify)]);
    }
    for (from, to) in transitions {
        for manual in [true, false] {
            let mut next = to.clone();
            next.index = 1;
            playback.play(vec![from.clone(), next], 0);
            assert_playing(&mut playback, 0, from);
            meter.wait(|peak| peak > AUDIBLE);
            let started_at = if manual {
                playback.toggle_pause();
                wait_until(&mut playback, START_DEADLINE, |p| {
                    p.status() == &Status::Paused
                });
                meter.wait(|peak| peak < AUDIBLE);
                let requested = Instant::now();
                playback.next();
                requested
            } else {
                let duration = playback.duration().unwrap();
                playback.seek(duration - 1.5);
                wait_until(&mut playback, START_DEADLINE, |p| {
                    p.current_index() == Some(1)
                });
                Instant::now()
            };
            // Let the meter observe sound independently of the 25 ms transport poll.
            let first_manual_peak =
                manual.then(|| meter.wait_for(START_DEADLINE, |peak| peak > AUDIBLE));
            let manual_ms = manual.then(|| started_at.elapsed().as_millis());
            assert_playing(&mut playback, 1, to);
            let peak = first_manual_peak.unwrap_or_else(|| meter.wait(|peak| peak > AUDIBLE));
            let wasapi_ms = manual_ms.unwrap_or_else(|| started_at.elapsed().as_millis());
            let engine_ms = playback.sound_details().time_to_sound_ms.unwrap();
            let scenario = if manual {
                "Next from pause to signal"
            } else {
                "EOF advance to confirmed output (upper bound)"
            };
            println!(
                "{scenario}: {} → {}, WASAPI {wasapi_ms} ms, engine {engine_ms} ms, peak={peak:.6}",
                source(from).label(),
                source(to).label()
            );
        }
    }
}

fn source(entry: &QueueEntry) -> SoundSource {
    match entry.playlist.provider().unwrap() {
        ProviderId::Spotify => SoundSource::Soloist,
        provider => SoundSource::Stream(provider),
    }
}

fn assert_playing(playback: &mut Playback, position: usize, entry: &QueueEntry) {
    wait_until(playback, START_DEADLINE, |p| {
        p.current_index() == Some(position) && p.status() == &Status::Playing && p.position() > 0.10
    });
    assert_eq!(
        playback.sound_source(),
        Some(&source(entry)),
        "the expected native source for the selected entry"
    );
}

fn recording(store: &Store, provider: ProviderId, name: &str, default: &str) -> QueueEntry {
    let id = std::env::var(name).unwrap_or_else(|_| default.to_owned());
    let account = store
        .preferred_account(provider)
        .unwrap()
        .expect("connected provider account");
    let credentials = store
        .credentials(&account)
        .unwrap()
        .expect("available provider account");
    let cancelled = AtomicBool::new(false);
    let tracks = if provider == ProviderId::SoundCloud {
        providers::read_playlist(provider, Some(&credentials), &id, &cancelled)
            .expect("SoundCloud test playlist")
            .tracks
    } else {
        catalog::read(
            &ObjectRef {
                provider,
                kind: ObjectKind::Track,
                id: id.clone(),
            },
            Some(&credentials),
            &cancelled,
        )
        .expect("catalog test track")
        .tracks
    };
    let track = tracks.into_iter()
        .find(|track| matches!(&track.media, Some(Media::Provider { provider: source, .. }) if *source == provider))
        .expect("playable native test track");
    QueueEntry {
        playlist: PlaylistKey::Source {
            provider,
            profile: Some(account.profile.id),
            id,
        },
        playlist_revision: None,
        recommendations: Vec::new(),
        index: 0,
        track,
    }
}
