use std::{
    path::PathBuf,
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};

use crate::{
    music::{
        ObjectKind, ObjectRef,
        lyrics::{self, Lyrics, Source},
    },
    playback::SoundSource,
    playlist::{Media, PlaylistKey},
    providers::{ProviderId, catalog},
    storage::Store,
};

use super::{Loopback, Playback, QueueEntry, Status, entry, wait_until};

const FRAME: Duration = Duration::from_micros(16_667);
const TOLERANCE_MS: u64 = 150;

#[test]
#[ignore = "requires libmpv, WASAPI and MEDIADEKA_AUDIO_FIXTURE"]
fn windows_stage6_lyrics_local_sync_and_seek() {
    let path = PathBuf::from(std::env::var_os("MEDIADEKA_AUDIO_FIXTURE").expect("fixture path"));
    let profile = tempfile::tempdir().unwrap();
    let file = profile.path().join("stage6.flac");
    std::fs::copy(path, &file).unwrap();
    std::fs::write(
        file.with_extension("lrc"),
        "[00:01.00]<00:01.00>Первая <00:01.50>строка<00:01.90>\n[00:02.00]Вторая строка\n[00:03.00]Третья строка",
    ).unwrap();
    let store = Store::open(profile.path()).unwrap();
    let item = entry(0, Some(Media::File(file)));
    let lyrics = load(&store, &item, Source::Local, SoundSource::LocalFile);
    assert_eq!(lyrics.lines[0].words.len(), 2);
    let cancelled = AtomicBool::new(true);
    lyrics::load(&store, &item, None, Some(Source::Local), &cancelled, |_| {
        panic!("cancelled request published lyrics")
    })
    .unwrap();
    check_playback(item, &lyrics);
}

#[test]
#[ignore = "requires libmpv, WASAPI and MEDIADEKA_SOUND_TEST_PROFILE with subscribed Yandex Music"]
fn windows_stage6_lyrics_yandex_sync_and_seek() {
    check_native(
        "windows_stage6_lyrics_yandex_sync_and_seek",
        ProviderId::YandexMusic,
        Source::YandexMusic,
        "10741527:1612097",
        "MEDIADEKA_LYRICS_YANDEX_TRACK",
    );
}

#[test]
#[ignore = "requires libmpv, WASAPI and MEDIADEKA_SOUND_TEST_PROFILE with Spotify and available sound"]
fn windows_stage6_lyrics_spotify_sync_and_seek() {
    check_native(
        "windows_stage6_lyrics_spotify_sync_and_seek",
        ProviderId::Spotify,
        Source::Spotify,
        "2Fxmhks0bxGSBdJ92vM42m",
        "MEDIADEKA_LYRICS_SPOTIFY_TRACK",
    );
}

fn check_native(
    test: &str,
    provider: ProviderId,
    source: Source,
    default_id: &str,
    override_id: &str,
) {
    const CHILD: &str = "MEDIADEKA_STAGE6_LYRICS_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let snapshot = crate::providers::live_tests::audio::stage5_profile();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("playback::tests::windows::lyrics::{test}"),
                "--ignored",
                "--nocapture",
            ])
            .env("MEDIADEKA_PROFILE_DIR", snapshot.path())
            .env(CHILD, "1")
            .status()
            .expect("launch the isolated lyrics playback check");
        assert!(status.success());
        return;
    }

    let store = Store::open_default().unwrap();
    let cancelled = AtomicBool::new(false);
    let id = std::env::var(override_id).unwrap_or_else(|_| default_id.into());
    let account = store
        .preferred_account(provider)
        .unwrap()
        .expect("connected account");
    let credentials = store
        .credentials(&account)
        .unwrap()
        .expect("account credentials");
    let reference = ObjectRef {
        provider,
        kind: ObjectKind::Track,
        id,
    };
    let page = catalog::read(&reference, Some(&credentials), &cancelled).expect("catalog track");
    let track = page.tracks.into_iter().next().expect("catalog track row");
    let item = QueueEntry {
        playlist: PlaylistKey::Source {
            provider,
            profile: Some(account.profile.id.clone()),
            id: format!("track:{}", reference.id),
        },
        playlist_revision: None,
        recommendations: Vec::new(),
        index: 0,
        track,
    };
    let lyrics = load(&store, &item, source, SoundSource::Stream(provider));
    assert_eq!(lyrics.source, source);
    let reopened = Store::open_default().unwrap();
    assert_eq!(
        load(&reopened, &item, source, SoundSource::Stream(provider)),
        lyrics
    );
    if provider == ProviderId::Spotify {
        crate::music::load_page(&reference, Some(&account.profile.id), &cancelled, |_| {});
    }
    println!(
        "{} · {} · {} lines",
        source.label(),
        item.track.title,
        lyrics.lines.len()
    );
    check_playback(item, &lyrics);
}

fn load(store: &Store, item: &QueueEntry, source: Source, sound: SoundSource) -> Lyrics {
    let mut loaded = None;
    lyrics::load(
        store,
        item,
        Some(&sound),
        Some(source),
        &AtomicBool::new(false),
        |versions| {
            loaded = Some(versions.into_preferred());
        },
    )
    .expect("load lyrics through the application resolver");
    loaded.expect("timed lyrics for the fixture")
}

fn check_playback(item: QueueEntry, lyrics: &Lyrics) {
    let meter = Loopback::start();
    let mut playback = Playback::new();
    playback.set_volume(0.15);
    wait_until(&mut playback, Duration::from_secs(8), |p| {
        (p.volume() - 0.15).abs() < 0.001
    });
    playback.play(vec![item], 0);
    wait_until(&mut playback, Duration::from_secs(35), |p| {
        p.status() == &Status::Playing && p.position() > 0.2
    });
    let peak = meter.wait(|peak| peak > 0.0001);
    assert_eq!(playback.frame.output.as_deref(), Some("wasapi"));

    let (index, time_ms) = lyrics
        .lines
        .iter()
        .enumerate()
        .find_map(|(index, line)| {
            let cue = line.time_ms?;
            let next = lyrics.lines.get(index + 1)?.time_ms?;
            (cue >= 1000 && next.saturating_sub(cue) > 2 * TOLERANCE_MS).then_some((index, cue))
        })
        .expect("measurable lyric cue");
    playback.seek((time_ms - 450) as f64 / 1000.0);
    wait_until(&mut playback, Duration::from_secs(8), |p| {
        (position_ms(p).abs_diff(time_ms - 450)) <= TOLERANCE_MS
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    let (position, active) = loop {
        playback.poll();
        let position = position_ms(&playback);
        let active = lyrics.active_line(position);
        if position >= time_ms {
            break (position, active);
        }
        assert!(
            Instant::now() < deadline,
            "lyrics cue did not advance, position={position}"
        );
        thread::sleep(FRAME);
    };
    assert_eq!(active, Some(index));
    let skew = position.abs_diff(time_ms);
    assert!(skew <= TOLERANCE_MS, "cue reached {skew} ms late");

    playback.toggle_pause();
    wait_until(&mut playback, Duration::from_secs(8), |p| {
        p.status() == &Status::Paused
    });
    let next = lyrics.lines[index + 1].time_ms.unwrap();
    let target = next + 30;
    playback.seek(target as f64 / 1000.0);
    wait_until(&mut playback, Duration::from_secs(8), |p| {
        position_ms(p).abs_diff(target) <= TOLERANCE_MS
            && lyrics.active_line(position_ms(p)) == Some(index + 1)
    });
    let sound = playback
        .sound_source()
        .expect("source of the current sound");
    println!(
        "{} · {} · cue {} ms ≤ {TOLERANCE_MS} ms · seek {target}→{} ms · WASAPI {peak:.6}",
        lyrics.source.label(),
        sound.label(),
        skew,
        position_ms(&playback),
    );
}

fn position_ms(playback: &Playback) -> u64 {
    (playback.position() * 1000.0).round() as u64
}
