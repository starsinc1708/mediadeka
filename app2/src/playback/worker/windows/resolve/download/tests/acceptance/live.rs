use std::{env, io::ErrorKind};

use crate::{
    music::{ConfirmedLink, ObjectKind, ObjectRef, region},
    providers::{self, LoginInput, Session, catalog},
};

use super::*;

const LIVE_CHILD: &str = "MEDIADEKA_STAGE5_LIVE_DOWNLOAD_RACE_CHILD";
const LIVE_TEST: &str = "windows_stage5_live_two_originals_upgrade_cache";

#[test]
#[ignore = "uses a connected SoundCloud profile, an isolated Jellyfin mirror, and WASAPI"]
fn windows_stage5_live_two_originals_upgrade_cache() {
    if !in_isolated_profile(LIVE_TEST, LIVE_CHILD) {
        return;
    }

    let expected_best = fs::read(fixture("MEDIADEKA_TEST_SOUNDCLOUD_ORIGINAL_FILE")).unwrap();
    let expected_first = fs::read(fixture("MEDIADEKA_TEST_JELLYFIN_ORIGINAL_FILE")).unwrap();
    let store = Store::open_default().unwrap();
    let cancelled = AtomicBool::new(false);
    let soundcloud = soundcloud_session();
    let jellyfin = providers::sign_in(
        ProviderId::Jellyfin,
        LoginInput::JellyfinPassword {
            server: fixture("MEDIADEKA_TEST_JELLYFIN_URL"),
            username: fixture("MEDIADEKA_TEST_JELLYFIN_USER"),
            password: fixture("MEDIADEKA_TEST_JELLYFIN_PASSWORD"),
        },
        &cancelled,
        |_| {},
    )
    .expect("isolated Jellyfin mirror");
    for session in [&soundcloud, &jellyfin] {
        store.connect_account(session, || Some(()), || {}).unwrap();
    }
    let entry = mirrored_recording(&store, &jellyfin, &cancelled);
    assert!(store.cached_audio(&entry).unwrap().is_none());

    let refusals = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let refusal_url = format!("http://{}/stream", listener.local_addr().unwrap());
    let worker = Worker::spawn_windows(0.2, move |sender, receiver, output, signals, volume| {
        run_driver(
            Driver {
                frame: Frame {
                    volume,
                    ..Frame::default()
                },
                signals,
                direct_failure_url: Some(refusal_url),
                ..Driver::default()
            },
            sender,
            receiver,
            output,
        );
    })
    .unwrap();

    let mut playback = Playback {
        worker: Some(worker),
        ..Playback::default()
    };
    let meter = Loopback::start();
    thread::scope(|scope| {
        let refusals = &refusals;
        let stop = &stop;
        scope.spawn(move || reject_direct(listener, stop, refusals));
        let _stop = StopListening(stop);

        playback.play(vec![entry.clone()], 0);
        wait_for_sound(&mut playback, |player| player.position() > 0.13);
        let event = playback
            .take_sound_started()
            .expect("real original playback");
        let peak = meter.wait(|peak| peak > 0.0001);
        assert!(
            refusals.load(Ordering::Acquire) >= 2,
            "both direct streams failed over HTTP"
        );
        assert_eq!(
            event.source,
            SoundSource::Download(ProviderId::Jellyfin),
            "the local lower-quality mirror should finish before SoundCloud"
        );
        let quality = event.details.quality.as_ref().unwrap();
        let began = playback.position();
        assert_eq!(quality.codec, "mp3");
        wait_for_sound(&mut playback, |player| player.cache_revision() >= 1);
        let first = store.cached_audio(&entry).unwrap().unwrap();
        if playback.cache_revision() == 1 {
            assert_eq!(fs::read(&first.path).unwrap(), expected_first);
        }
        wait_for_sound(&mut playback, |player| {
            player.cache_revision() == 2 && player.position() > began
        });
        let best = store.cached_audio(&entry).unwrap().unwrap();
        assert_eq!(fs::read(&best.path).unwrap(), expected_best);
        assert!(best.quality.rank() > quality.rank());
        assert_eq!(playback.sound_source(), Some(&event.source));
        assert_eq!(playback.audio_quality().unwrap(), quality);
        assert!(playback.take_sound_started().is_none());
        store.record_sound(&event).unwrap();
        println!(
            "Two providers: {} started ({}, {:?} ms, WASAPI peak={peak:.6}, position={began:.3}s); the exact SoundCloud original ({}) replaced cache while Jellyfin kept playing",
            event.source.label(),
            quality.label(),
            event.details.time_to_sound_ms,
            best.quality.label()
        );
    });
    drop(playback);
    for session in [&soundcloud, &jellyfin] {
        store
            .disconnect_account(session.provider, &session.profile.id, || Some(()))
            .unwrap();
    }

    let mut offline = Playback::new();
    offline.set_volume(0.2);
    offline.play(vec![entry], 0);
    wait_for_sound(&mut offline, |player| player.position() > 0.15);
    assert_eq!(offline.sound_source(), Some(&SoundSource::Cached));
    let peak = meter.wait(|peak| peak > 0.0001);
    println!(
        "Disconnected replay: {}, position={:.3}s, WASAPI peak={peak:.6}",
        offline.audio_quality().unwrap().label(),
        offline.position()
    );
}

fn fixture(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("missing {name}"))
}

fn soundcloud_session() -> Session {
    let snapshot = providers::live_tests::audio::stage5_profile();
    let source = Store::open(snapshot.path()).unwrap();
    let account = source
        .preferred_account(ProviderId::SoundCloud)
        .unwrap()
        .expect("connected SoundCloud account");
    let credentials = source.credentials(&account).unwrap().unwrap();
    Session {
        provider: account.provider,
        profile: account.profile,
        credentials,
        capabilities: account.capabilities,
    }
}

fn mirrored_recording(store: &Store, jellyfin: &Session, cancelled: &AtomicBool) -> QueueEntry {
    let source = ObjectRef {
        provider: ProviderId::Jellyfin,
        kind: ObjectKind::Track,
        id: fixture("MEDIADEKA_TEST_JELLYFIN_ITEM"),
    };
    let url = fixture("MEDIADEKA_TEST_SOUNDCLOUD_URL");
    let target = ObjectRef::from_url(&url)
        .filter(|item| item.provider == ProviderId::SoundCloud && item.kind == ObjectKind::Track)
        .unwrap();
    let tracks = catalog::read(&source, Some(&jellyfin.credentials), cancelled)
        .unwrap()
        .tracks;
    let track = tracks.into_iter().next().unwrap();
    let link = ConfirmedLink {
        platform: ProviderId::SoundCloud.as_str().into(),
        id: target.id,
        url,
        evidence: "Тестовая копия того же оригинала SoundCloud".into(),
        checked_at: 1,
    };
    let region = region();
    assert!(store.claim_music_match(&region, &source, 0, 1).unwrap());
    store
        .save_music_match(
            &region,
            std::slice::from_ref(&source),
            1,
            2,
            &serde_json::json!({ "links": [link] }),
        )
        .unwrap();
    QueueEntry {
        playlist: PlaylistKey::Source {
            provider: source.provider,
            profile: Some(jellyfin.profile.id.clone()),
            id: format!("track:{}", source.id),
        },
        playlist_revision: None,
        recommendations: Vec::new(),
        index: 0,
        track,
    }
}

struct StopListening<'a>(&'a AtomicBool);
impl Drop for StopListening<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn reject_direct(listener: TcpListener, stop: &AtomicBool, refusals: &AtomicUsize) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                assert_eq!(
                    request(&stream).first().map(String::as_str),
                    Some("GET /stream HTTP/1.1")
                );
                refusals.fetch_add(1, Ordering::Release);
                stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("isolated direct refusal socket: {error}"),
        }
    }
}

fn wait_for_sound(playback: &mut Playback, ready: impl Fn(&Playback) -> bool) {
    poll_until(playback, Duration::from_secs(40), ready);
}
