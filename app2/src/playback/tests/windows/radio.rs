use std::{collections::BTreeSet, env, process::Command};

use crate::music::{
    ObjectKind, ObjectRef,
    recommendations::{
        Mode,
        acceptance::copy_profile,
        radio::{self, Seed},
        wave::Access,
    },
};

use super::*;

const LIVE_CHILD: &str = "MEDIADEKA_AUTOPLAY_AUDIO_CHILD";

#[test]
#[ignore = "plays a real Yandex Music station in an isolated Windows profile and records WASAPI"]
fn windows_ordinary_queue_autoplay_wasapi() {
    if env::var_os(LIVE_CHILD).is_none() {
        let (profile, _) = copy_profile();
        let status = Command::new(env::current_exe().unwrap())
            .args([
                "windows_ordinary_queue_autoplay_wasapi",
                "--ignored",
                "--nocapture",
            ])
            .env("MEDIADEKA_PROFILE_DIR", profile.path())
            .env(LIVE_CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }

    let fixture = PathBuf::from(env::var_os("MEDIADEKA_AUDIO_FIXTURE").unwrap());
    let store = Store::open_default().unwrap();
    let cancelled = AtomicBool::new(false);
    let provider = ProviderId::YandexMusic;
    let seed = provider_track(&store, provider, "149723835:41349305", &cancelled);
    let meter = Loopback::start();
    let ordinary = vec![entry(0, Some(Media::File(fixture))), seed.clone()];
    let mut playback = Playback::new();
    playback.set_volume(0.14);
    wait(&mut playback, |p| (p.volume() - 0.14).abs() < 0.001);
    playback.play(ordinary.clone(), 0);
    wait(&mut playback, |p| {
        p.status() == &Status::Playing && p.current_index() == Some(0)
    });
    assert!(playback.continuation_point().is_none());
    playback.seek(playback.duration().unwrap() - 0.35);
    wait(&mut playback, |p| {
        p.status() == &Status::Playing && p.current_index() == Some(1) && p.position() > 0.35
    });
    assert!(playback.continuation_point().is_none());
    assert_eq!(playback.frame.output.as_deref(), Some("wasapi"));
    println!(
        "Ordinary queue: {} seed {:?} sounded after the manual local entry.",
        provider.display_name(),
        seed.track.title
    );

    playback.seek(playback.duration().unwrap() - 0.5);
    wait(&mut playback, |p| p.continuation_point().is_some());
    let point = playback.continuation_point().unwrap();
    assert!(!playback.has_queued_next());
    assert_eq!(playback.status(), &Status::Idle);
    assert_eq!(playback.entries().len(), 2);

    let batch = radio::start(
        &store,
        &Seed::Track(Box::new(seed.clone())),
        Mode::Familiar,
        Access::Live,
        &cancelled,
    )
    .unwrap();
    let queued = playback.entries().cloned().collect::<Vec<_>>();
    let recommendations = batch.entries(&queued);
    let first = recommendations
        .first()
        .expect("radio returned no playable recommendations");
    let expected = first.track.id.clone();
    let origin = first.recommendations[0].playlist.clone();
    assert_ne!(first.track.id, seed.track.id);
    assert_eq!(first.recommendations[0].provider, provider);
    println!(
        "At natural EOF: position {:.3}s, {} recommendations from {:?}, first={expected}.",
        playback.position(),
        recommendations.len(),
        origin
    );

    playback.continue_after(point, recommendations);
    wait(&mut playback, |p| {
        p.current_index() == Some(2) && p.status() == &Status::Playing && p.position() > 0.5
    });
    assert_eq!(playback.current().unwrap().track.id, expected);
    assert_eq!(playback.frame.output.as_deref(), Some("wasapi"));
    let peak = meter.wait(|value| value > 0.0001);
    let started = playback.take_sound_started().unwrap();
    println!(
        "WASAPI autoplay: row 3, first candidate {expected}, {}, position {:.3}s, peak {peak:.6}.",
        started.source.label(),
        playback.position()
    );
    drop(playback);
    meter.wait(|value| value < 0.0001);
    crate::ui::check_automatic_queue(ordinary, || meter.wait(|value| value > 0.0001));
}

#[test]
#[ignore = "uses a recording already confirmed across the three current connected catalogs"]
fn contextual_radio_partial_provider_failure() {
    let (_copy, store) = copy_profile();
    let cancelled = AtomicBool::new(false);
    let seed = provider_track(
        &store,
        ProviderId::YandexMusic,
        "145242789:39226560",
        &cancelled,
    );
    let live = radio::start(
        &store,
        &Seed::Track(Box::new(seed.clone())),
        Mode::Familiar,
        Access::Live,
        &cancelled,
    )
    .unwrap();
    let providers = |entries: &[QueueEntry]| {
        entries
            .iter()
            .flat_map(|entry| &entry.recommendations)
            .map(|origin| origin.provider)
            .collect::<BTreeSet<_>>()
    };
    let live_providers = providers(live.candidates());
    println!(
        "Live confirmed recording {:?}: {:?}; unavailable={:?}.",
        seed.track.title, live_providers, live.unavailable
    );
    assert!(live_providers.contains(&ProviderId::YandexMusic));
    assert!(
        live_providers.contains(&ProviderId::SoundCloud)
            || live_providers.contains(&ProviderId::Spotify)
    );
    let saved = radio::start(
        &store,
        &Seed::Track(Box::new(seed.clone())),
        Mode::Familiar,
        Access::Saved,
        &cancelled,
    )
    .unwrap();
    let excluded = [seed];
    let entries = saved.entries(&excluded);
    let remaining = providers(&entries);
    assert!(!remaining.is_empty());
    assert!(saved.unavailable.contains(&ProviderId::YandexMusic));
    assert!(!remaining.contains(&ProviderId::YandexMusic));
    println!(
        "Without live station access: {} recommendations from {remaining:?}, unavailable={:?}.",
        entries.len(),
        saved.unavailable
    );
}

fn provider_track(
    store: &Store,
    provider: ProviderId,
    id: &str,
    cancelled: &AtomicBool,
) -> QueueEntry {
    let account = store
        .preferred_account(provider)
        .unwrap()
        .expect("connected profile");
    let credentials = store
        .credentials(&account)
        .unwrap()
        .expect("valid saved access");
    let object = ObjectRef {
        provider,
        kind: ObjectKind::Track,
        id: id.into(),
    };
    let page = providers::catalog::read(&object, Some(&credentials), cancelled).unwrap();
    QueueEntry {
        playlist: PlaylistKey::Source {
            provider,
            profile: Some(account.profile.id),
            id: object.id,
        },
        playlist_revision: None,
        index: 0,
        track: page
            .tracks
            .into_iter()
            .next()
            .expect("recording from provider"),
        recommendations: Vec::new(),
    }
}
