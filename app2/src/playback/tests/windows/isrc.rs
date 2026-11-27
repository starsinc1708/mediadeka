use std::{
    io::{BufRead, BufReader, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use lofty::{file::AudioFile as _, probe::Probe};
use serde_json::{Value, json};

use crate::{
    music::{ObjectKind, ObjectRef, region},
    playback::SoundSource,
    providers::{
        Capabilities, Credentials, Profile, ProviderId, Session,
        live_tests::audio::decode_fixture_spotify_track, yandex::test_server::use_loopback,
    },
};

use super::{Loopback, Playback, PlaylistKey, QueueEntry, Status, Store, wait};

const CHILD: &str = "MEDIADEKA_STAGE5_ISRC_FIXTURE_CHILD";
const TITLE: &str = "Stage five signal";
const ARTIST: &str = "Mediadeka fixture";
const ISRC: &str = "USABC2400123";
const SPOTIFY_ID: &str = "0000000000000000000001";
const YANDEX_TOKEN: &str = "mediadeka-fixture-yandex";
const EXPECTED_YANDEX_TRACK: &str = "303";
const AUDIO_PATH: &str = "/fixture/audio";

#[test]
#[ignore = "requires libmpv, WASAPI and a local FLAC/MP3 tone in MEDIADEKA_AUDIO_FIXTURE"]
fn windows_stage5_isrc_resolves_through_http_and_wasapi() {
    if std::env::var_os(CHILD).is_none() {
        let directory = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "playback::tests::windows::isrc::windows_stage5_isrc_resolves_through_http_and_wasapi",
                "--ignored",
                "--nocapture",
            ])
            .env("MEDIADEKA_PROFILE_DIR", directory.path())
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }

    let audio = PathBuf::from(std::env::var_os("MEDIADEKA_AUDIO_FIXTURE").expect("fixture audio"));
    let codec = match audio.extension().and_then(|extension| extension.to_str()) {
        Some("flac") => "flac",
        Some("mp3") => "mp3",
        _ => panic!("expected a FLAC or MP3 fixture"),
    };
    let properties = Probe::open(&audio).unwrap().read().unwrap();
    let duration = properties.properties().duration();
    assert!(duration > Duration::from_secs(2));
    let duration_ms = i32::try_from(duration.as_millis()).unwrap();
    let server = FixtureServer::start(std::fs::read(&audio).unwrap(), codec, duration_ms);
    use_loopback(server.address);

    let profile = Store::profile_directory().expect("isolated profile directory");
    assert!(!Store::uses_default_profile());
    assert!(std::fs::read_dir(&profile).unwrap().next().is_none());
    let store = Store::open(profile).expect("isolated profile");
    let accounts = FixtureAccounts::new(&store);
    let object = ObjectRef {
        provider: ProviderId::Spotify,
        kind: ObjectKind::Track,
        id: SPOTIFY_ID.into(),
    };
    let page = decode_fixture_spotify_track(&object, TITLE, ARTIST, "US-ABC-24-00123", duration_ms);
    assert_eq!(page.isrc.as_deref(), Some("US-ABC-24-00123"));
    let spotify = store
        .account(ProviderId::Spotify, &accounts.profiles[0].1)
        .unwrap()
        .unwrap();
    assert!(
        store
            .save_music_snapshot(
                ProviderId::Spotify,
                Some(&spotify),
                &region(),
                &format!("track:{SPOTIFY_ID}"),
                &page,
            )
            .unwrap()
    );
    let entry = QueueEntry {
        playlist: PlaylistKey::Source {
            provider: ProviderId::Spotify,
            profile: Some(spotify.profile.id),
            id: "fixture-collection".into(),
        },
        playlist_revision: None,
        recommendations: Vec::new(),
        index: 0,
        track: page.tracks.into_iter().next().unwrap(),
    };

    let meter = Loopback::start();
    let mut playback = Playback::new();
    playback.set_volume(0.3);
    playback.play(vec![entry.clone()], 0);
    wait(&mut playback, |player| {
        player.status() == &Status::Playing && player.position() > 0.2
    });
    let peak = meter.wait(|peak| peak > 0.0001);
    let started = playback.take_sound_started().expect("first playing frame");
    assert_eq!(started.source, SoundSource::Stream(ProviderId::YandexMusic));
    assert_eq!(started.entry.track.id, SPOTIFY_ID);
    assert_eq!(playback.frame.output.as_deref(), Some("wasapi"));
    let reason = started.details.reason.as_deref().unwrap();
    assert!(
        reason.contains("Совпадение ISRC") && reason.contains("Spotify"),
        "{reason}"
    );
    let quality = playback.audio_quality().expect("quality from libmpv");
    assert_eq!(quality.codec, codec);
    assert_eq!(started.details.quality.as_ref(), Some(quality));
    assert!(store.cached_audio(&entry).unwrap().is_none());
    store.record_sound(&started).unwrap();
    let history = store.listening_history().unwrap();
    assert_eq!(history[0].platform, Some(ProviderId::Spotify));
    assert_eq!(history[0].source, started.source);
    assert_eq!(history[0].details, started.details);

    let requests = server.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|path| path.as_str() != AUDIO_PATH)
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "/search",
            "/tracks/303:501",
            "/account/status",
            "/get-file-info/303"
        ]
    );
    assert!(requests.iter().any(|path| path == AUDIO_PATH));
    println!(
        "Fixture Spotify protobuf + Yandex HTTP {ISRC} → Yandex track {EXPECTED_YANDEX_TRACK}, {} · position={:.3}s · WASAPI peak={peak:.6} · startup={}ms",
        quality.label(),
        playback.position(),
        started.details.time_to_sound_ms.unwrap(),
    );
}

struct FixtureAccounts<'a> {
    store: &'a Store,
    profiles: [(ProviderId, String); 2],
}

impl<'a> FixtureAccounts<'a> {
    fn new(store: &'a Store) -> Self {
        let fixture = Self {
            store,
            profiles: [
                (
                    ProviderId::Spotify,
                    format!("fixture-spotify-{}", std::process::id()),
                ),
                (
                    ProviderId::YandexMusic,
                    format!("fixture-yandex-{}", std::process::id()),
                ),
            ],
        };
        let credentials = [
            Credentials::Spotify {
                client_id: "fixture".into(),
                access_token: "fixture".into(),
                refresh_token: "fixture".into(),
                access_expires_at: None,
                librespot: None,
            },
            Credentials::YandexMusic {
                access_token: YANDEX_TOKEN.into(),
                refresh_token: None,
                access_expires_at: None,
            },
        ];
        for ((provider, id), credentials) in fixture.profiles.iter().zip(credentials) {
            let session = Session {
                provider: *provider,
                profile: Profile {
                    id: id.clone(),
                    name: id.clone(),
                },
                capabilities: Capabilities {
                    home: true,
                    profile_selection: false,
                    device_code: false,
                },
                credentials,
            };
            assert!(
                store
                    .connect_account(&session, || Some(()), || {})
                    .unwrap()
                    .is_some()
            );
        }
        fixture
    }
}

impl Drop for FixtureAccounts<'_> {
    fn drop(&mut self) {
        for (provider, id) in &self.profiles {
            let _ = self.store.disconnect_account(*provider, id, || Some(()));
        }
    }
}

struct FixtureServer {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl FixtureServer {
    fn start(audio: Vec<u8>, codec: &'static str, duration_ms: i32) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let cancelled = Arc::clone(&stop);
        let evidence = Arc::clone(&requests);
        let worker = thread::spawn(move || {
            while !cancelled.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        serve(stream, address, &audio, codec, duration_ms, &evidence)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("fixture listener: {error}"),
                }
            }
        });
        Self {
            address,
            stop,
            requests,
            worker: Some(worker),
        }
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let result = self.worker.take().unwrap().join();
        if !thread::panicking() {
            result.unwrap();
        }
    }
}

fn serve(
    mut stream: TcpStream,
    address: SocketAddr,
    audio: &[u8],
    codec: &str,
    duration_ms: i32,
    requests: &Mutex<Vec<String>>,
) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut lines = BufReader::new(&stream).lines();
    let request = lines.next().unwrap().unwrap();
    let path = request.split_whitespace().nth(1).unwrap();
    let url = url::Url::parse(&format!("http://{address}{path}")).unwrap();
    let mut authenticated = false;
    for line in lines
        .map_while(Result::ok)
        .take_while(|line| !line.is_empty())
    {
        authenticated |= line.split_once(':').is_some_and(|(key, value)| {
            key.eq_ignore_ascii_case("authorization")
                && value.trim() == format!("OAuth {YANDEX_TOKEN}")
        });
    }
    if url.path() == AUDIO_PATH {
        requests.lock().unwrap().push(AUDIO_PATH.into());
        write_response(&mut stream, "200 OK", &format!("audio/{codec}"), audio);
        return;
    }
    assert!(
        authenticated,
        "fixture catalog must use the connected profile"
    );
    let track_id = url
        .query_pairs()
        .find(|(name, _)| name == "trackId")
        .map(|(_, id)| id);
    let route = track_id.as_ref().map_or_else(
        || url.path().to_owned(),
        |id| format!("{}/{id}", url.path()),
    );
    requests.lock().unwrap().push(route.clone());
    let response = match route.as_str() {
        "/search" => json!({"result":{"tracks":{"results":[
            track(301, Some("USABC2400124"), duration_ms),
            track(302, Some(ISRC), duration_ms + 5_000),
            track(303, None, duration_ms)
        ]}}}),
        "/tracks/303:501" => json!({"result":[track(303, Some("usabc2400123"), duration_ms)]}),
        "/account/status" => json!({"result":{"plus":{"hasPlus":true}}}),
        "/get-file-info/303" => json!({"result":{"downloadInfo":{
            "codec":codec,"transport":"raw","preview":false,
            "url":format!("http://{address}{AUDIO_PATH}")
        }}}),
        _ => {
            write_response(
                &mut stream,
                "404 Not Found",
                "text/plain",
                b"fixture route absent",
            );
            return;
        }
    };
    write_response(
        &mut stream,
        "200 OK",
        "application/json",
        response.to_string().as_bytes(),
    );
}

fn track(id: u32, isrc: Option<&str>, duration_ms: i32) -> Value {
    json!({
        "id": id,
        "title": TITLE,
        "artists": [{"id":1,"name":ARTIST}],
        "albums": [{"id":501,"title":"Fixture"}],
        "available": true,
        "durationMs": duration_ms,
        "isrc": isrc
    })
}

fn write_response(stream: &mut TcpStream, status: &str, content_type: &str, bytes: &[u8]) {
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).unwrap();
    stream.write_all(bytes).unwrap();
}
