mod general_wave;
mod isrc;
mod lyrics;
mod radio;
mod transitions;

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use wasapi::{AudioClient, Direction, SampleType, StreamMode, WasapiError, WaveFormat};

use crate::{
    playback::{CollectionKey, CollectionPlan},
    playlist::{Media, PlaylistKey},
    providers::{self, ProviderId},
    storage::Store,
};

use super::{Playback, QueueEntry, Status, entry};

#[test]
#[ignore = "requires libmpv and MEDIADEKA_AUDIO_FIXTURE with at least three seconds of audio"]
fn windows_collection_resume_after_store_reopen() {
    let fixture =
        PathBuf::from(std::env::var_os("MEDIADEKA_AUDIO_FIXTURE").expect("audio fixture"));
    let profile = tempfile::tempdir().unwrap();
    let store = Store::open(profile.path()).unwrap();
    let id = store
        .create_local_playlist("Продолжить", &[fixture.clone(), fixture])
        .unwrap();
    let key = PlaylistKey::Local(id);
    let playlist = store.local_playlist(id).unwrap().unwrap();
    let plan = Arc::new(CollectionPlan::new(
        key.clone(),
        &playlist.name,
        None,
        &playlist.tracks,
    ));
    let entries = plan.entries.clone();
    let mut playback = Playback::new();
    playback.play_collection(entries, 0, plan);
    wait(&mut playback, |p| {
        p.status() == &Status::Playing && p.position() > 0.2
    });
    let duration = playback.duration().unwrap();
    playback.seek(duration - 0.3);
    wait(&mut playback, |p| {
        p.current().unwrap().index == 1 && p.status() == &Status::Playing && p.position() > 0.8
    });
    playback.toggle_pause();
    wait(&mut playback, |p| p.status() == &Status::Paused);
    let update = playback.take_collection_progress().unwrap();
    assert!(update.progress.fraction() > 0.5 && update.progress.fraction() < 1.0);
    assert!(update.progress.completed(0) && !update.progress.completed(1));
    store.save_collection_listening(&update, true).unwrap();
    drop(playback);
    drop(store);

    let store = Store::open(profile.path()).unwrap();
    let (plan, progress) = store
        .collection_listening(CollectionKey::Playlist(key))
        .unwrap()
        .unwrap();
    let target = progress.position_ms as f64 / 1_000.;
    assert!(target > 0.7 && progress.resumable());
    let mut restored = Playback::new();
    let meter = Loopback::start();
    restored.set_volume(0.2);
    wait(&mut restored, |p| (p.volume() - 0.2).abs() < 0.001);
    restored.resume_collection(Arc::new(plan), progress);
    wait(&mut restored, |p| p.status() == &Status::Playing);
    assert_eq!(restored.current().unwrap().index, 1);
    assert_eq!(restored.entries().len(), 2);
    assert!((restored.position() - target).abs() < 0.25);
    assert_eq!(restored.frame.output.as_deref(), Some("wasapi"));
    let peak = meter.wait(|value| value > 0.0001);
    println!(
        "WASAPI collection checkpoint {target:.3}s returned to index 1 at {:.3}s after reopening SQLite; peak {peak:.6}.",
        restored.position(),
    );
}

#[test]
#[ignore = "requires real WASAPI audio output and MEDIADEKA_AUDIO_FIXTURE"]
fn windows_playlist_audio_and_queue() {
    let fixture = PathBuf::from(std::env::var_os("MEDIADEKA_AUDIO_FIXTURE").unwrap());
    assert!(fixture.is_absolute() && fixture.is_file());
    let make = |index| entry(index, Some(Media::File(fixture.clone())));
    let meter = Loopback::start();
    let mut playback = Playback::new();
    playback.request_devices();
    wait(&mut playback, |p| !p.devices().is_empty());
    assert_eq!(playback.audio_device(), "auto");
    assert!(
        playback
            .devices()
            .iter()
            .any(|device| device.id.starts_with("wasapi/"))
    );

    playback.set_volume(0.35);
    wait(&mut playback, |p| (p.volume() - 0.35).abs() < 0.001);
    // Recreate the worker with the last confirmed transport settings.
    playback.worker = None;
    playback.play(vec![make(0), make(1), make(2)], 1);
    assert_eq!(playback.status(), &Status::Loading);
    wait(&mut playback, |p| {
        p.status() == &Status::Playing && p.position() > 0.2
    });
    assert_eq!(playback.frame.output.as_deref(), Some("wasapi"));
    assert!((playback.volume() - 0.35).abs() < 0.001);
    let first_peak = meter.wait(|peak| peak > 0.001);
    let initial_position = playback.position();
    assert_eq!(playback.current().unwrap().index, 1);

    playback.toggle_pause();
    wait(&mut playback, |p| p.status() == &Status::Paused);
    let paused_peak = meter.wait(|peak| peak < 0.0001);
    // WASAPI may still be draining the last hardware buffer after pause is acknowledged.
    thread::sleep(Duration::from_millis(150));
    playback.poll();
    let paused_position = playback.position();
    thread::sleep(Duration::from_millis(230));
    playback.poll();
    assert!(
        (playback.position() - paused_position).abs() < 0.065,
        "paused_position={paused_position:.3}, later_position={:.3}, status={:?}",
        playback.position(),
        playback.status(),
    );
    assert!(playback.seekable());
    let duration = playback.duration().unwrap();
    let target = duration / 2.;
    playback.seek(target);
    wait(&mut playback, |p| (p.position() - target).abs() < 0.25);
    assert_eq!(playback.status(), &Status::Paused);
    let sought_position = playback.position();
    playback.toggle_pause();
    wait(&mut playback, |p| {
        p.status() == &Status::Playing && p.position() > sought_position + 0.15
    });
    let resumed_peak = meter.wait(|peak| peak > 0.001);

    playback.insert_next(vec![make(3)]);
    playback.append(vec![make(4)]);
    assert_eq!(
        playback
            .entries()
            .map(|entry| entry.index)
            .collect::<Vec<_>>(),
        [0, 1, 3, 2, 4]
    );
    playback.seek(duration - 0.20);
    wait(&mut playback, |p| {
        p.current().is_some_and(|item| item.index == 3) && p.status() == &Status::Playing
    });
    assert!(playback.can_previous());
    assert!(playback.can_next());
    playback.previous();
    wait(&mut playback, |p| {
        p.current().is_some_and(|item| item.index == 1) && p.status() == &Status::Playing
    });

    playback.cycle_repeat();
    playback.cycle_repeat();
    playback.seek(duration - 0.40);
    wait(&mut playback, |p| p.position() > duration - 0.65);
    let generation = playback.generation;
    wait(&mut playback, |p| {
        p.generation != generation && p.status() == &Status::Playing
    });
    assert_eq!(playback.current().unwrap().index, 1);

    playback.toggle_pause();
    wait(&mut playback, |p| p.status() == &Status::Paused);
    meter.wait(|peak| peak < 0.0001);
    let next_requested = Instant::now();
    playback.next();
    let next_peak = meter.wait(|peak| peak > 0.001);
    let next_audible = next_requested.elapsed();
    wait(&mut playback, |p| {
        p.current().is_some_and(|item| item.index == 3) && p.status() == &Status::Playing
    });
    let next_engine_ms = playback.sound_details().time_to_sound_ms.unwrap();
    println!(
        "WASAPI manual next from a paused track: signal in {} ms, engine confirmation in {next_engine_ms} ms, peak={next_peak:.6}.",
        next_audible.as_millis(),
    );

    let missing = fixture.with_extension("missing-audio-file");
    playback.play(vec![entry(5, Some(Media::File(missing)))], 0);
    assert_eq!(playback.status(), &Status::Loading);
    wait(&mut playback, |p| matches!(p.status(), Status::Failed(_)));
    let error = format!("{:?}", playback.status());
    playback.play(vec![make(6)], 0);
    wait(&mut playback, |p| {
        p.status() == &Status::Playing && p.position() > 0.20
    });
    let recovered_peak = meter.wait(|peak| peak > 0.001);
    let (codec, decoded_rate, output_rate) = playback.frame.test_audio_format.as_ref().unwrap();
    assert!(!codec.is_empty() && *decoded_rate > 0. && *output_rate > 0.);

    println!(
        "WASAPI driver=wasapi devices={} codec={codec} decoded_rate={decoded_rate:.0}Hz output_rate={output_rate:.0}Hz initial_position={initial_position:.3}s initial_peak={first_peak:.6} paused_position={paused_position:.3}s paused_peak={paused_peak:.6} sought_position={sought_position:.3}s resumed_peak={resumed_peak:.6} duration={duration:.3}s auto_next_index=3 previous_index=1 repeat_one_index=1 error={error} recovered_peak={recovered_peak:.6}",
        playback.devices().len(),
    );
}

#[test]
#[ignore = "requires a dedicated test profile and playlist for MEDIADEKA_TEST_AUDIO_PROVIDER"]
fn windows_provider_playlist_audio() {
    assert!(
        std::env::var_os("MEDIADEKA_PROFILE_DIR").is_some(),
        "set the isolated profile"
    );
    let provider = std::env::var("MEDIADEKA_TEST_AUDIO_PROVIDER").expect("set the audio provider");
    let provider = ProviderId::ALL
        .into_iter()
        .find(|id| id.as_str() == provider)
        .expect("known provider");
    let playlist_id = std::env::var("MEDIADEKA_TEST_AUDIO_PLAYLIST").expect("set the playlist ID");
    let store = Store::open_default().expect("open the isolated profile");
    let account = store
        .preferred_account(provider)
        .expect("read preferred account")
        .expect("test account");
    let credentials = store
        .credentials(&account)
        .expect("read credentials")
        .expect("test credential");
    let cancelled = AtomicBool::new(false);
    let playlist = providers::read_playlist(provider, Some(&credentials), &playlist_id, &cancelled)
        .expect("read the live playlist");
    let requested_track = std::env::var("MEDIADEKA_TEST_AUDIO_TRACK")
        .ok()
        .filter(|id| !id.is_empty());
    let (index, track) = playlist
        .tracks
        .into_iter()
        .enumerate()
        .find(|(_, row)| {
            matches!(&row.media, Some(Media::Provider { .. }))
                && requested_track.as_deref().is_none_or(|id| row.id == id)
        })
        .expect("the requested provider-backed track");
    println!("Selected {} · {} ({})", track.artist, track.title, track.id);
    let playlist = PlaylistKey::Source {
        provider,
        profile: Some(account.profile.id),
        id: playlist_id,
    };
    let meter = Loopback::start();
    let mut playback = Playback::new();
    playback.set_volume(0.15);
    wait(&mut playback, |p| (p.volume() - 0.15).abs() < 0.001);
    playback.play(
        vec![QueueEntry {
            playlist_revision: None,
            recommendations: Vec::new(),
            playlist,
            index,
            track,
        }],
        0,
    );
    wait_until(&mut playback, Duration::from_secs(35), |p| {
        p.status() == &Status::Playing && p.position() > 0.20
    });
    let peak = meter.wait(|peak| peak > 0.0001);
    let position = playback.position();
    let (codec, decoded_rate, output_rate) = playback
        .frame
        .test_audio_format
        .clone()
        .expect("confirmed audio format");
    assert_eq!(playback.frame.output.as_deref(), Some("wasapi"));
    playback.toggle_pause();
    wait(&mut playback, |p| p.status() == &Status::Paused);
    let paused_peak = meter.wait(|value| value < peak / 4.);
    println!(
        "{} WASAPI codec={codec} decoded_rate={decoded_rate:.0}Hz output_rate={output_rate:.0}Hz position={position:.3}s peak={peak:.6} paused_peak={paused_peak:.6}",
        provider.display_name()
    );
}

#[test]
#[ignore = "requires WASAPI, libmpv and connected Spotify and subscribed Yandex Music in MEDIADEKA_SOUND_TEST_PROFILE"]
fn windows_stage5_direct_and_isrc_fallback() {
    check_stage5_streams(
        "windows_stage5_direct_and_isrc_fallback",
        crate::providers::live_tests::audio::Stage5Evidence::Isrc,
        "Совпадение ISRC",
    );
}

#[test]
#[ignore = "requires WASAPI, libmpv and connected Spotify and subscribed Yandex Music in MEDIADEKA_SOUND_TEST_PROFILE"]
fn windows_stage5_direct_and_confirmed_fallback() {
    check_stage5_streams(
        "windows_stage5_direct_and_confirmed_fallback",
        crate::providers::live_tests::audio::Stage5Evidence::ConfirmedLink,
        "Подтверждённая музыкальная ссылка",
    );
}

fn check_stage5_streams(
    test: &str,
    evidence: crate::providers::live_tests::audio::Stage5Evidence,
    fallback_reason: &str,
) {
    use crate::{
        playback::SoundSource,
        providers::live_tests::audio::{stage5_profile, stage5_recordings},
    };

    const CHILD: &str = "MEDIADEKA_STAGE5_WASAPI_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let snapshot = stage5_profile();
        let connection = rusqlite::Connection::open(snapshot.path().join("mediadeka.sqlite3"))
            .expect("open disposable copy");
        // Exercise either matching path with a fresh link index.
        connection.execute("DELETE FROM music_match", []).unwrap();
        drop(connection);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("playback::tests::windows::{test}"),
                "--ignored",
                "--nocapture",
            ])
            .env("MEDIADEKA_PROFILE_DIR", snapshot.path())
            .env(CHILD, "1")
            .status()
            .expect("launch the isolated WASAPI playback check");
        assert!(status.success());
        return;
    }

    let store = Store::open_default().expect("isolated profile");
    let pair = stage5_recordings(&store, evidence, &AtomicBool::new(false));
    let meter = Loopback::start();
    let mut playback = Playback::new();
    playback.set_volume(0.15);
    wait(&mut playback, |p| (p.volume() - 0.15).abs() < 0.001);
    for (entry, expected) in [
        (pair.yandex, "Полный поток исходной площадки"),
        (pair.spotify, fallback_reason),
    ] {
        let origin = entry.playlist.provider().unwrap();
        let elapsed = Instant::now();
        assert!(store.cached_audio(&entry).unwrap().is_none());
        playback.play(vec![entry], 0);
        wait_until(&mut playback, Duration::from_secs(35), |p| {
            p.status() == &Status::Playing && p.position() > 0.2
        });
        let peak = meter.wait(|peak| peak > 0.0001);
        assert_eq!(playback.frame.output.as_deref(), Some("wasapi"));
        let started = playback.take_sound_started().expect("first playing frame");
        assert_eq!(started.source, SoundSource::Stream(ProviderId::YandexMusic));
        assert!(store.cached_audio(&started.entry).unwrap().is_none());
        let reason = started.details.reason.as_deref().expect("selection reason");
        assert!(reason.contains(expected), "{reason}");
        let quality = playback.audio_quality().expect("quality from libmpv");
        let time_to_sound = started.details.time_to_sound_ms.expect("time to sound");
        assert_eq!(started.details.quality.as_ref(), Some(quality));
        let (codec, decoded_rate, _) = playback.frame.test_audio_format.as_ref().unwrap();
        assert_eq!(&quality.codec, codec);
        assert_eq!(f64::from(quality.sample_rate_hz), *decoded_rate);
        println!(
            "{} track ISRC {} · {} · {} · to_sound={time_to_sound}ms · checked={}ms · position={:.3}s · WASAPI peak={peak:.6}",
            origin.display_name(),
            pair.isrc,
            started.source.label(),
            quality.label(),
            elapsed.elapsed().as_millis(),
            playback.position(),
        );
        playback.toggle_pause();
        wait(&mut playback, |p| p.status() == &Status::Paused);
        meter.wait(|value| value < 0.0001);
    }
}

#[test]
#[ignore = "requires WASAPI and a connected Yandex Music account in an isolated MEDIADEKA_PROFILE_DIR"]
fn windows_yandex_station_audio_and_refill() {
    use crate::music::station;

    assert!(std::env::var_os("MEDIADEKA_PROFILE_DIR").is_some());
    let profile = Store::open_default()
        .unwrap()
        .preferred_account(ProviderId::YandexMusic)
        .unwrap()
        .expect("connected Yandex Music account")
        .profile
        .id;
    let key = PlaylistKey::Station {
        provider: ProviderId::YandexMusic,
        profile: Some(profile),
        id: "user:onyourwave".into(),
    };
    let cancelled = AtomicBool::new(false);
    let first = station::start(&key, &cancelled).expect("start a Yandex station");
    let rotor = first.rotor.unwrap();
    let recent: Vec<_> = first.tracks.iter().map(|track| track.id.clone()).collect();
    assert!(!recent.is_empty());
    let entries = |tracks: Vec<_>, offset| {
        tracks
            .into_iter()
            .enumerate()
            .map(|(index, track)| QueueEntry {
                playlist: key.clone(),
                playlist_revision: Some(1),
                recommendations: Vec::new(),
                index: offset + index,
                track,
            })
            .collect::<Vec<_>>()
    };
    let meter = Loopback::start();
    let mut playback = Playback::new();
    playback.play(entries(first.tracks, 0), 0);
    wait(&mut playback, |p| {
        p.status() == &Status::Playing && p.position() > 0.2
    });
    assert_eq!(playback.frame.output.as_deref(), Some("wasapi"));
    assert_eq!(playback.take_sound_started().unwrap().entry.playlist, key);
    let peak = meter.wait(|peak| peak > 0.001);

    let fresh = station::refill(&key, &rotor.session_id, &recent, &cancelled)
        .expect("refill the same Yandex station");
    assert!(!fresh.tracks.is_empty());
    assert!(fresh.tracks.iter().all(|track| !recent.contains(&track.id)));
    let added = fresh.tracks.len();
    playback.append_continuation(entries(fresh.tracks, recent.len()));
    assert_eq!(playback.entries().len(), recent.len() + added);
    playback.play_queued(recent.len());
    wait(&mut playback, |p| {
        p.status() == &Status::Playing && p.position() > 0.2
    });
    assert_eq!(playback.current().unwrap().index, recent.len());
    let next_peak = meter.wait(|peak| peak > 0.001);
    println!(
        "Yandex station WASAPI: initial={}, added={added}, peaks={peak:.6}/{next_peak:.6}",
        recent.len()
    );
    let PlaylistKey::Station {
        provider, profile, ..
    } = key
    else {
        unreachable!()
    };
    for (kind, id) in [
        ("track", format!("track:{}", recent[0])),
        ("artist", "artist:36800".to_owned()),
    ] {
        let radio = PlaylistKey::Station {
            provider,
            profile: profile.clone(),
            id,
        };
        let tracks = station::start(&radio, &cancelled)
            .unwrap_or_else(|error| panic!("start Yandex {kind} radio: {error}"))
            .tracks;
        assert!(tracks.iter().any(|track| track.media.is_some()));
        println!("Yandex {kind} radio: tracks={}", tracks.len());
    }
}

#[test]
#[ignore = "requires WASAPI and a connected SoundCloud account with cached stations in an isolated MEDIADEKA_PROFILE_DIR"]
fn windows_soundcloud_artist_station_audio() {
    use crate::{
        music::{ObjectKind, ObjectRef, station},
        playback::SoundSource,
        storage::CacheScope,
    };

    const SEEK_SECONDS: u32 = 35;

    assert!(std::env::var_os("MEDIADEKA_PROFILE_DIR").is_some());
    let provider = ProviderId::SoundCloud;
    let store = Store::open_default().unwrap();
    let profile = store
        .preferred_account(provider)
        .unwrap()
        .unwrap()
        .profile
        .id;
    let snapshot = store
        .cache(CacheScope::Account(provider, &profile))
        .unwrap()
        .unwrap();
    let (object, link) = snapshot
        .home
        .items
        .iter()
        .filter_map(|item| {
            let object = ObjectRef::from_item(provider, item)?;
            (object.kind == ObjectKind::Station).then_some((object, item.url.as_deref()?))
        })
        .next()
        .expect("cached SoundCloud station with a public link");
    assert_eq!(ObjectRef::from_url(link).as_ref(), Some(&object));
    let key = PlaylistKey::Station {
        provider,
        profile: Some(profile),
        id: object.id,
    };
    let feed = station::start(&key, &AtomicBool::new(false)).expect("real SoundCloud station");
    let (index, track) = feed
        .tracks
        .into_iter()
        .enumerate()
        .find(|(_, track)| {
            let is_provider_recording =
                matches!(track.media.as_ref(), Some(Media::Provider { .. }));
            is_provider_recording
                && track
                    .duration
                    .is_some_and(|seconds| seconds > SEEK_SECONDS + 5)
        })
        .expect("SoundCloud station with a playable recording longer than the seek target");
    let meter = Loopback::start();
    let mut playback = Playback::new();
    playback.play(
        vec![QueueEntry {
            playlist: key,
            playlist_revision: Some(1),
            recommendations: Vec::new(),
            index,
            track,
        }],
        0,
    );
    wait(&mut playback, |p| {
        p.status() == &Status::Playing && p.position() > 0.2
    });
    assert_eq!(playback.frame.output.as_deref(), Some("wasapi"));
    let started = playback
        .take_sound_started()
        .expect("first audible recording");
    assert!(matches!(
        started.source,
        SoundSource::Stream(ProviderId::SoundCloud) | SoundSource::Download(ProviderId::SoundCloud)
    ));
    let first_peak = meter.wait(|peak| peak > 0.001);
    drop(meter);
    wait(&mut playback, Playback::seekable);
    playback.seek(f64::from(SEEK_SECONDS));
    wait(&mut playback, |p| {
        p.status() == &Status::Playing && p.position() > f64::from(SEEK_SECONDS) + 0.2
    });
    let seek_peak = Loopback::start().wait(|peak| peak > 0.001);
    println!(
        "SoundCloud station: source={}, position={:.3}s, WASAPI peaks={first_peak:.6}/{seek_peak:.6}",
        started.source.label(),
        playback.position()
    );
}

#[test]
#[ignore = "requires WASAPI, libmpv and isolated Jellyfin with its stream route disabled"]
fn windows_stage5_original_plays_when_stream_fails() {
    use crate::{
        music::{ObjectKind, ObjectRef},
        playback::SoundSource,
        providers::LoginInput,
    };

    const CHILD: &str = "MEDIADEKA_STAGE5_ORIGINAL_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let directory = tempfile::tempdir().unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "windows_stage5_original_plays_when_stream_fails",
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
    let fixture = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("missing {name}"));
    let provider = ProviderId::Jellyfin;
    let cancelled = AtomicBool::new(false);
    let session = providers::sign_in(
        provider,
        LoginInput::JellyfinPassword {
            server: fixture("MEDIADEKA_TEST_JELLYFIN_URL"),
            username: fixture("MEDIADEKA_TEST_JELLYFIN_USER"),
            password: fixture("MEDIADEKA_TEST_JELLYFIN_PASSWORD"),
        },
        &cancelled,
        |_| {},
    )
    .expect("isolated Jellyfin sign-in");
    let object = ObjectRef {
        provider,
        kind: ObjectKind::Track,
        id: fixture("MEDIADEKA_TEST_JELLYFIN_ITEM"),
    };
    let page = providers::catalog::read(&object, Some(&session.credentials), &cancelled).unwrap();
    let track = page.tracks.into_iter().next().unwrap();
    assert!(track.duration.is_some());
    let store = Store::open_default().unwrap();
    store.connect_account(&session, || Some(()), || {}).unwrap();
    let entry = QueueEntry {
        playlist: PlaylistKey::Source {
            provider,
            profile: Some(session.profile.id.clone()),
            id: format!("track:{}", track.id),
        },
        playlist_revision: None,
        recommendations: Vec::new(),
        index: 0,
        track,
    };
    let meter = Loopback::start();
    let mut playback = Playback::new();
    playback.set_volume(0.3);
    playback.play(vec![entry.clone()], 0);
    wait(&mut playback, |player| {
        player.sound_source() == Some(&SoundSource::Download(provider))
            && player.position() > 0.15
            && player.cache_revision() > 0
    });
    let started = playback.take_sound_started().unwrap();
    store.record_sound(&started).unwrap();
    let peak = meter.wait(|peak| peak > 0.0001);
    let cached = store.cached_audio(&entry).unwrap().unwrap();
    let quality = playback.audio_quality().unwrap();
    assert_eq!(cached.quality.codec, quality.codec);
    assert_eq!(cached.quality.sample_rate_hz, quality.sample_rate_hz);
    println!(
        "WASAPI original peak={peak:.6}, position={:.3}s, source={}, {} after {:?} ms; verified cache persisted",
        playback.position(),
        started.source.label(),
        quality.label(),
        started.details.time_to_sound_ms
    );
    drop(playback);
    store
        .disconnect_account(provider, &session.profile.id, || Some(()))
        .unwrap();
}

fn wait(playback: &mut Playback, predicate: impl Fn(&Playback) -> bool) {
    wait_until(playback, Duration::from_secs(8), predicate);
}

fn wait_until(playback: &mut Playback, timeout: Duration, predicate: impl Fn(&Playback) -> bool) {
    let deadline = Instant::now() + timeout;
    loop {
        playback.poll();
        if predicate(playback) {
            return;
        }
        assert!(
            !matches!(playback.status(), Status::Failed(_)),
            "{:?}",
            playback.status()
        );
        assert!(
            Instant::now() < deadline,
            "status={:?}, position={}, output={:?}, error={:?}",
            playback.status(),
            playback.position(),
            playback.frame.output,
            playback.error()
        );
        thread::sleep(Duration::from_millis(25));
    }
}

pub(in crate::playback) struct Loopback {
    peak: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<Result<(), WasapiError>>>,
}

impl Loopback {
    pub(in crate::playback) fn start() -> Self {
        let peak = Arc::new(AtomicU32::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let meter = Arc::clone(&peak);
        let cancel = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            wasapi::initialize_mta().ok()?;
            let result = capture(&meter, &cancel);
            wasapi::deinitialize();
            result
        });
        Self {
            peak,
            stop,
            worker: Some(worker),
        }
    }

    pub(in crate::playback) fn wait(&self, matching: impl Fn(f32) -> bool) -> f32 {
        self.wait_for(Duration::from_secs(5), matching)
    }

    fn wait_for(&self, timeout: Duration, matching: impl Fn(f32) -> bool) -> f32 {
        let deadline = Instant::now() + timeout;
        loop {
            let peak = f32::from_bits(self.peak.load(Ordering::Relaxed));
            if matching(peak) {
                return peak;
            }
            assert!(Instant::now() < deadline, "WASAPI peak={peak:.6}");
            thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.unwrap().unwrap();
            }
        }
    }
}

fn capture(peak: &AtomicU32, stop: &AtomicBool) -> Result<(), WasapiError> {
    let event;
    let mut client = AudioClient::new_application_loopback_client(std::process::id(), true)?;
    let format = WaveFormat::new(32, 32, SampleType::Float, 48_000, 2, None);
    client.initialize_client(
        &format,
        Direction::Capture,
        StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: 0,
        },
        None,
    )?;
    event = client.set_get_eventhandle()?;
    let capture = client.get_audiocaptureclient()?;
    let stride = format.get_blockalign() as usize;
    let mut bytes = Vec::new();
    client.start_stream()?;
    let mut last_packet = Instant::now();
    while !stop.load(Ordering::Acquire) {
        match event.wait_for_event(40) {
            Ok(()) | Err(WasapiError::EventTimeout) => {}
            Err(error) => return Err(error),
        }
        let mut maximum = 0.0_f32;
        let mut received = false;
        while let Some(frames @ 1..) = capture.get_next_packet_size()? {
            received = true;
            bytes.resize(frames as usize * stride, 0);
            let (frames, info) = capture.read_from_device(&mut bytes)?;
            if !info.flags.silent {
                maximum = bytes[..frames as usize * stride]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|value| f32::from_le_bytes(*value).abs())
                    .filter(|value| value.is_finite())
                    .fold(maximum, f32::max);
            }
        }
        if received {
            last_packet = Instant::now();
            peak.store(maximum.to_bits(), Ordering::Relaxed);
        } else if last_packet.elapsed() > Duration::from_millis(100) {
            peak.store(0, Ordering::Relaxed);
        }
    }
    client.stop_stream()
}
