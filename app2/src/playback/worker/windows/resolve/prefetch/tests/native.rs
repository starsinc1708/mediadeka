use std::{
    env, fs,
    io::{self, BufRead, BufReader, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    process::Command,
    sync::mpsc::{self, Receiver},
};

use serde_json::json;

use crate::{
    playback::{Playback, SoundSource, Status, tests::windows::Loopback},
    playlist::PlaylistKey,
    providers::{Capabilities, Credentials, Profile, Session, yandex::test_server},
    storage::Store,
};

use super::*;

const CHILD: &str = "MEDIADEKA_AUDIO_PREFETCH_CHILD";
const TEST: &str = "playback::worker::windows::resolve::prefetch::tests::native::windows_audio_prefetch_follows_queue_edits_and_reuses_the_ready_stream";
const PROFILE: &str = "74368";
const FIRST: usize = 101;
const MOVED: usize = 102;
const FINAL: usize = 103;
const AUDIBLE: f32 = 0.001;

#[test]
#[ignore = "requires libmpv, WASAPI and real local WAV / streamed FLAC media fixtures"]
fn windows_audio_prefetch_follows_queue_edits_and_reuses_the_ready_stream() {
    if env::var_os(CHILD).is_none() {
        let profile = tempfile::tempdir().unwrap();
        let status = Command::new(env::current_exe().unwrap())
            .args([TEST, "--exact", "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("MEDIADEKA_PROFILE_DIR", profile.path())
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let wave = PathBuf::from(env::var_os("MEDIADEKA_AUDIO_FIXTURE").unwrap());
    let flac = PathBuf::from(env::var_os("MEDIADEKA_PREFETCH_STREAM_FIXTURE").unwrap());
    let evidence = PathBuf::from(env::var_os("MEDIADEKA_AUDIO_PREFETCH_EVIDENCE").unwrap());
    assert!(wave.is_absolute() && wave.is_file());
    let bytes = fs::read(flac).unwrap();
    assert!(bytes.starts_with(b"fLaC"));
    fs::create_dir_all(&evidence).unwrap();

    let store = Store::open_default().unwrap();
    store
        .connect_account(
            &Session {
                provider: ProviderId::YandexMusic,
                profile: Profile {
                    id: PROFILE.into(),
                    name: "Queue source fixture".into(),
                },
                credentials: Credentials::YandexMusic {
                    access_token: "fixture-yandex-queue-source".into(),
                    refresh_token: None,
                    access_expires_at: None,
                },
                capabilities: Capabilities {
                    home: true,
                    profile_selection: false,
                    device_code: true,
                },
            },
            || Some(()),
            || {},
        )
        .unwrap()
        .unwrap();
    let _account = AccountGuard(store);

    let (signal, events) = mpsc::channel();
    EVENTS.set(signal).unwrap();
    let fixture = Fixture::start(bytes);
    let meter = Loopback::start();
    let mut player = Playback::new();
    player.set_volume(0.3);
    poll_until(&mut player, "volume", |player| {
        (player.volume() - 0.3).abs() < 0.001
    });

    player.play(
        vec![
            entry(0, Some(Media::File(wave.clone()))),
            entry(1, Some(Media::File(wave))),
            stream_entry(FIRST),
            stream_entry(MOVED),
            stream_entry(FINAL),
        ],
        0,
    );
    poll_until(&mut player, "local source", |player| {
        player.status() == &Status::Playing && player.position() > 0.20
    });
    assert_eq!(player.sound_source(), Some(&SoundSource::LocalFile));
    assert_eq!(player.frame.output.as_deref(), Some("wasapi"));
    let local_peak = meter.wait(|peak| peak > AUDIBLE);
    assert!(fixture.requests.lock().unwrap().is_empty());
    let current = player.take_sound_started().unwrap();
    assert_eq!(current.entry.index, 0);

    remove(&mut player, 1);
    fixture.wait_for(&mut player, Request::Source(FIRST));
    let (id, _) = player.queued(2).unwrap();
    assert!(player.move_upcoming(player.queue_revision(), id, 2, 1));
    wait_event(&events, Event::Cancelled(FIRST));
    fixture.release_first.send(()).unwrap();
    wait_event(
        &events,
        Event::Settled {
            index: FIRST,
            ready: false,
        },
    );

    fixture.wait_for(&mut player, Request::Source(MOVED));
    remove(&mut player, 2); // The demoted track cannot reclaim the only slot.
    remove(&mut player, 1); // Delete the running speculative resolution.
    wait_event(&events, Event::Cancelled(MOVED));
    fixture.release_moved.send(()).unwrap();
    wait_event(
        &events,
        Event::Settled {
            index: MOVED,
            ready: false,
        },
    );
    fixture.wait_for(&mut player, Request::Source(FINAL));
    wait_event(
        &events,
        Event::Settled {
            index: FINAL,
            ready: true,
        },
    );

    poll_until(&mut player, "the first generation continues", |player| {
        player.position() > 0.35
    });
    assert_eq!(player.generation, current.generation);
    assert_eq!(player.current().unwrap().index, 0);
    assert_eq!(player.sound_source(), Some(&SoundSource::LocalFile));
    assert!(player.take_sound_started().is_none());
    assert_eq!(player.status(), &Status::Playing);
    let continuous_peak = meter.wait(|peak| peak > AUDIBLE);
    let local_position = player.position();
    let descriptors = [
        Request::Source(FIRST),
        Request::Source(MOVED),
        Request::Source(FINAL),
    ];
    assert_eq!(*fixture.requests.lock().unwrap(), descriptors);

    player.toggle_pause();
    poll_until(&mut player, "pause before transition", |player| {
        player.status() == &Status::Paused
    });
    meter.wait(|peak| peak < 0.0001);
    let next = Instant::now();
    player.next();
    let stream_peak = meter.wait(|peak| peak > AUDIBLE);
    let wasapi_time = next.elapsed();
    poll_until(&mut player, "prepared source becoming audible", |player| {
        player.status() == &Status::Playing && player.position() > 0.20
    });
    assert_eq!(player.current().unwrap().index, FINAL);
    assert_eq!(player.queued(1).unwrap().1.index, FINAL);
    assert_eq!(
        player.sound_source(),
        Some(&SoundSource::Stream(ProviderId::YandexMusic))
    );
    assert_eq!(player.frame.output.as_deref(), Some("wasapi"));
    let media_requests = fixture.requests.lock().unwrap().clone();
    assert_eq!(&media_requests[..descriptors.len()], descriptors.as_slice());
    assert!(media_requests.len() > descriptors.len());
    assert!(
        media_requests[descriptors.len()..]
            .iter()
            .all(|request| *request == Request::Media(FINAL))
    );

    let engine_time = player.sound_details().time_to_sound_ms.unwrap();
    let report = format!(
        "WASAPI local peak={local_peak:.6}, after edits={continuous_peak:.6}, same generation at {local_position:.3}s\nPlayback candidates: none behind the next local WAV; cancelled and discarded {FIRST}, {MOVED}; prepared {FINAL}\nHTTP order: {media_requests:?}\nExplicit Next: stream started using the prepared source, WASAPI {} ms, engine {engine_time} ms, peak={stream_peak:.6}\n",
        wasapi_time.as_millis()
    );
    fs::write(evidence.join("prepared-audio.txt"), &report).unwrap();
    print!("{report}");
}

struct AccountGuard(Store);

impl Drop for AccountGuard {
    fn drop(&mut self) {
        let _ = self
            .0
            .disconnect_account(ProviderId::YandexMusic, PROFILE, || Some(()));
    }
}

fn stream_entry(index: usize) -> QueueEntry {
    let mut item = recording(index);
    item.playlist = PlaylistKey::Source {
        provider: ProviderId::YandexMusic,
        profile: Some(PROFILE.into()),
        id: "74368:880".into(),
    };
    item.track.id = index.to_string();
    item
}

fn remove(player: &mut Playback, position: usize) {
    let (id, _) = player.queued(position).unwrap();
    assert!(player.remove_upcoming(player.queue_revision(), id, position));
}

fn wait_event(events: &Receiver<Event>, expected: Event) {
    assert_eq!(
        events.recv_timeout(Duration::from_secs(8)).unwrap(),
        expected
    );
}

fn poll_until(player: &mut Playback, stage: &str, ready: impl Fn(&Playback) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        player.poll();
        if ready(player) {
            return;
        }
        assert!(
            Instant::now() < deadline && !matches!(player.status(), Status::Failed(_)),
            "{stage}: status={:?}, position={:.3}, reason={:?}",
            player.status(),
            player.position(),
            player.sound_details().reason
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Request {
    Source(usize),
    Media(usize),
}

struct Fixture {
    requests: Arc<Mutex<Vec<Request>>>,
    release_first: Sender<()>,
    release_moved: Sender<()>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn start(audio: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        test_server::use_loopback(address);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        let (release_first, first) = mpsc::channel();
        let (release_moved, moved) = mpsc::channel();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => answer(stream, address, &audio, &seen, &first, &moved),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(4))
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        });
        Self {
            requests,
            release_first,
            release_moved,
            stop,
            worker: Some(worker),
        }
    }

    fn wait_for(&self, player: &mut Playback, expected: Request) {
        poll_until(player, &format!("HTTP {expected:?}"), |_| {
            self.requests.lock().unwrap().contains(&expected)
        });
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.release_first.send(());
        let _ = self.release_moved.send(());
        self.stop.store(true, Ordering::Release);
        let result = self.worker.take().unwrap().join();
        if !thread::panicking() {
            result.unwrap();
        }
    }
}

fn answer(
    mut stream: TcpStream,
    address: SocketAddr,
    audio: &[u8],
    requests: &Mutex<Vec<Request>>,
    first: &Receiver<()>,
    moved: &Receiver<()>,
) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(4)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(4)))
        .unwrap();
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap() == 0 {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap();
    let url = url::Url::parse(&format!("http://{address}{path}")).unwrap();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
            break;
        }
    }
    if let Some(id) = url
        .path()
        .strip_prefix("/audio/")
        .and_then(|path| path.strip_suffix(".flac"))
        .and_then(|id| id.parse::<usize>().ok())
    {
        requests.lock().unwrap().push(Request::Media(id));
        response(&mut stream, "200 OK", "audio/flac", audio);
        return;
    }
    let body = match url.path() {
        "/account/status" => json!({"result":{"plus":{"hasPlus":true}}}),
        "/get-file-info" => {
            let id = url
                .query_pairs()
                .find(|(key, _)| key == "trackId")
                .unwrap()
                .1
                .parse::<usize>()
                .unwrap();
            let mut seen = requests.lock().unwrap();
            let is_first = !seen.contains(&Request::Source(id));
            seen.push(Request::Source(id));
            drop(seen);
            if is_first
                && let Some(gate) = match id {
                    FIRST => Some(first),
                    MOVED => Some(moved),
                    _ => None,
                }
            {
                gate.recv_timeout(Duration::from_secs(8)).unwrap();
            }
            json!({"result":{"downloadInfo":{"codec":"flac","transport":"raw","preview":false,
                "url":format!("http://{address}/audio/{id}.flac")}}})
        }
        _ => {
            response(
                &mut stream,
                "404 Not Found",
                "text/plain",
                b"fixture route absent",
            );
            return;
        }
    };
    response(
        &mut stream,
        "200 OK",
        "application/json",
        body.to_string().as_bytes(),
    );
}

fn response(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
        .and_then(|_| stream.write_all(body));
}
