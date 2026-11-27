mod live;

use std::{
    fs,
    io::Write,
    net::TcpListener,
    path::PathBuf,
    process::Command as Process,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use sha1::{Digest, Sha1};

use crate::{
    playback::{
        Playback, Status,
        tests::windows::Loopback,
        worker::{
            Frame, Worker,
            windows::{
                Driver,
                resolve::{Resolve, Resolver},
                run_driver,
            },
        },
    },
    playlist::{Media, PlaylistKey, PlaylistTrack},
    providers::{ProviderId, downloads::DownloadCandidate},
    storage::Store,
};

use super::{
    super::{Candidate, Plan, QueueEntry, SoundSource},
    request,
};

const CHILD: &str = "MEDIADEKA_STAGE5_DOWNLOAD_RACE_CHILD";
const FIXTURES: &str = "MEDIADEKA_STAGE5_DOWNLOAD_RACE_FIXTURES";
const DURATION_SECONDS: u32 = 14;
const START_LIMIT: Duration = Duration::from_secs(8);

#[test]
#[ignore = "requires libmpv, WASAPI, and generated MP3 and FLAC race fixtures"]
fn windows_stage5_two_originals_start_then_improve_and_replay_offline() {
    if !in_isolated_profile(
        "windows_stage5_two_originals_start_then_improve_and_replay_offline",
        CHILD,
    ) {
        return;
    }

    let directory = PathBuf::from(std::env::var_os(FIXTURES).expect("race fixture directory"));
    let mp3 = fs::read(directory.join("first.mp3")).unwrap();
    let flac = fs::read(directory.join("better.flac")).unwrap();
    let (fast, first) = serve_file(&mp3);
    let (slower, better) = serve_file(&flac);
    let store = Store::open_default().unwrap();
    let entry = input();
    let started_requests = AtomicUsize::new(0);
    let release_better = AtomicBool::new(false);
    let (pending, requests) = async_channel::bounded::<Resolve>(1);
    let (messages, signal) = mpsc::sync_channel(1);
    let worker = Worker::spawn_windows(0.2, move |sender, input, output, signals, volume| {
        messages.send(sender.clone()).unwrap();
        run_driver(
            Driver {
                frame: Frame {
                    volume,
                    ..Frame::default()
                },
                resolver: Some(Resolver(pending)),
                signals,
                ..Driver::default()
            },
            sender,
            input,
            output,
        );
    })
    .unwrap();
    let sender = signal.recv_timeout(START_LIMIT).unwrap();
    let mut playback = Playback {
        worker: Some(worker),
        ..Playback::default()
    };
    let meter = Loopback::start();
    thread::scope(|scope| {
        scope.spawn(|| {
            serve(fast, &mp3, &started_requests, None);
        });
        scope.spawn(|| {
            serve(slower, &flac, &started_requests, Some(&release_better));
        });
        scope.spawn(|| {
            let request = requests
                .recv_blocking()
                .expect("normal queue source request");
            assert_eq!(request.entry.track, entry.track);
            let original = |_: &Store, provider, _: &str, _: &str, _: &AtomicBool| {
                let fixture = match provider {
                    ProviderId::SoundCloud => &first,
                    ProviderId::Jellyfin => &better,
                    _ => unreachable!("unexpected fixture provider"),
                };
                let mut candidate = DownloadCandidate::new(fixture.url.clone());
                candidate.bytes = fixture.bytes;
                candidate.sha1 = fixture.sha1;
                Ok(candidate)
            };
            Plan {
                store: store.clone(),
                entry: request.entry,
                generation: request.generation,
                cancelled: request.cancelled,
                candidates: vec![
                    Candidate::Provider {
                        provider: ProviderId::SoundCloud,
                        profile: "fixture-soundcloud".into(),
                        id: "101".into(),
                    },
                    Candidate::Provider {
                        provider: ProviderId::Jellyfin,
                        profile: "fixture-jellyfin".into(),
                        id: "e8ea82a7f21e4a2cb54119c541bba4f9".into(),
                    },
                ],
                notes: vec!["Изолированный стенд, прямой поток недоступен".into()],
                setup_failure: false,
            }
            .run(&sender, original);
        });

        playback.play(vec![entry.clone()], 0);
        poll_until(&mut playback, START_LIMIT, |player| {
            player.position() > 0.12 && player.cache_revision() == 1
        });
        assert_eq!(
            started_requests.load(Ordering::Acquire),
            2,
            "both HTTP sources started before the first sound"
        );
        let started = playback.take_sound_started().expect("queue journal event");
        assert_eq!(
            started.source,
            SoundSource::Download(ProviderId::SoundCloud)
        );
        assert_eq!(started.details.quality.as_ref().unwrap().codec, "mp3");
        let first_started_at = started.details.time_to_sound_ms.unwrap();
        let initial = store.cached_audio(&entry).unwrap().unwrap();
        assert_eq!(fs::read(&initial.path).unwrap(), mp3);
        let first_position = playback.position();
        let first_peak = meter.wait(|peak| peak > 0.0001);
        store.record_sound(&started).unwrap();

        release_better.store(true, Ordering::Release);
        poll_until(&mut playback, START_LIMIT, |player| {
            player.cache_revision() == 2 && player.position() > first_position
        });
        let upgraded = store.cached_audio(&entry).unwrap().unwrap();
        assert!(upgraded.quality.rank() > initial.quality.rank());
        assert_eq!(fs::read(&upgraded.path).unwrap(), flac);
        assert_eq!(playback.audio_quality().unwrap().codec, "mp3");
        assert_eq!(playback.sound_source(), Some(&started.source));
        assert!(
            playback.take_sound_started().is_none(),
            "updating cache must not restart playback"
        );
        let history = store.listening_history().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].source, started.source);
        assert_eq!(history[0].details, started.details);
        println!(
            "Both originals requested first; WASAPI MP3 peak={first_peak:.6}, position={first_position:.3}s, startup={first_started_at}ms; FLAC upgraded cache while the same MP3 kept playing"
        );
    });
    // A fresh ordinary worker reads the cache with no provider connections.
    drop(playback);
    let mut offline = Playback::new();
    offline.set_volume(0.2);
    offline.play(vec![entry], 0);
    poll_until(&mut offline, START_LIMIT, |player| {
        player.position() > 0.15 && player.sound_source() == Some(&SoundSource::Cached)
    });
    assert_eq!(offline.audio_quality().unwrap().codec, "flac");
    let peak = meter.wait(|peak| peak > 0.0001);
    println!(
        "WASAPI offline cache FLAC peak={peak:.6}, position={:.3}s",
        offline.position()
    );
}

fn input() -> QueueEntry {
    let id = "e8ea82a7f21e4a2cb54119c541bba4f9";
    QueueEntry {
        playlist: PlaylistKey::Source {
            provider: ProviderId::Jellyfin,
            profile: Some("fixture-jellyfin".into()),
            id: "stage5-download-race".into(),
        },
        playlist_revision: None,
        recommendations: Vec::new(),
        index: 0,
        track: PlaylistTrack {
            id: id.into(),
            title: "Stage 5 acceptance recording".into(),
            artist: "Mediadeka".into(),
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: None,
            artwork: None,
            added: None,
            duration: Some(DURATION_SECONDS),
            media: Some(Media::Provider {
                provider: ProviderId::Jellyfin,
                id: id.into(),
            }),
        },
    }
}

fn serve_file(bytes: &[u8]) -> (TcpListener, DownloadCandidate) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = url::Url::parse(&format!(
        "http://{}/original",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let mut candidate = DownloadCandidate::new(url);
    candidate.sha1 = Some(Sha1::digest(bytes).into());
    candidate.bytes = Some(bytes.len() as u64);
    (listener, candidate)
}

fn serve(listener: TcpListener, bytes: &[u8], started: &AtomicUsize, gate: Option<&AtomicBool>) {
    let deadline = Instant::now() + START_LIMIT;
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("waiting for original HTTP request: {error}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(START_LIMIT)).unwrap();
    stream.set_write_timeout(Some(START_LIMIT)).unwrap();
    assert_eq!(
        request(&stream).first().map(String::as_str),
        Some("GET /original HTTP/1.1")
    );
    started.fetch_add(1, Ordering::Release);
    while started.load(Ordering::Acquire) != 2
        || gate.is_some_and(|gate| !gate.load(Ordering::Acquire))
    {
        assert!(
            Instant::now() < deadline,
            "waiting for parallel requests and first playback"
        );
        thread::sleep(Duration::from_millis(10));
    }
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"stage5-original\"\r\nConnection: close\r\n\r\n", bytes.len()).unwrap();
    stream.write_all(bytes).unwrap();
}

fn poll_until(playback: &mut Playback, timeout: Duration, ready: impl Fn(&Playback) -> bool) {
    let deadline = Instant::now() + timeout;
    while !ready(playback) {
        playback.poll();
        assert!(
            !matches!(playback.status(), Status::Failed(_)),
            "{:?}",
            playback.status()
        );
        assert!(
            Instant::now() < deadline,
            "playback: {:?}, source: {:?}, position: {}",
            playback.status(),
            playback.sound_source(),
            playback.position()
        );
        thread::sleep(Duration::from_millis(20));
    }
}
fn in_isolated_profile(test: &str, marker: &str) -> bool {
    if std::env::var_os(marker).is_some() {
        return true;
    }
    let profile = tempfile::tempdir().unwrap();
    let child = Process::new(std::env::current_exe().unwrap())
        .args([test, "--ignored", "--nocapture"])
        .env(marker, "1")
        .env("MEDIADEKA_PROFILE_DIR", profile.path())
        .output()
        .unwrap();
    print!("{}", String::from_utf8_lossy(&child.stdout));
    eprint!("{}", String::from_utf8_lossy(&child.stderr));
    assert!(child.status.success());
    false
}
