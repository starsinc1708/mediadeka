use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::atomic::{AtomicBool, Ordering},
    thread::{self, JoinHandle},
    time::{Duration, Instant, UNIX_EPOCH},
};

use md5::{Digest, Md5};
use serde_json::{Value, json};
use url::Url;

use super::{LastFmAccount, ListenBrainzAccount, Play, audioscrobbler, listenbrainz};

#[test]
fn sonoras_listen_threshold_uses_half_the_track_or_four_minutes() {
    let mut play = played();
    assert!(!play.earned(Duration::from_millis(20_249)));
    assert!(play.earned(Duration::from_millis(20_250)));
    play.duration = Duration::from_millis(29_999);
    assert!(!play.earned(Duration::from_secs(240)));
    play.duration = Duration::from_secs(30);
    assert!(play.earned(Duration::from_secs(15)));
    play.duration = Duration::from_secs(800);
    assert!(!play.earned(Duration::from_millis(239_999)));
    assert!(play.earned(Duration::from_secs(240)));
}

#[test]
fn last_fm_signs_approval_exchange_now_playing_and_scrobble() {
    let (endpoint, server) = server(&[
        (200, r#"{"token":"preflight-token"}"#),
        (
            200,
            r#"{"session":{"key":"test-session","name":"listener"}}"#,
        ),
        (200, r#"{"nowplaying":{"ignoredMessage":{"code":"0"}}}"#),
        (200, r#"{"scrobbles":{"@attr":{"accepted":1,"ignored":0}}}"#),
    ]);
    let endpoint = format!("{endpoint}/2.0/");
    let mut browser = None;
    let account = audioscrobbler::connect_at(
        &endpoint,
        "http://127.0.0.1/approve",
        0,
        " test-key ".to_owned(),
        " test-secret ".to_owned(),
        &AtomicBool::new(false),
        |authorize| {
            let authorize = Url::parse(authorize).unwrap();
            assert_eq!(authorize.path(), "/approve");
            let params: BTreeMap<_, _> = authorize.query_pairs().into_owned().collect();
            assert_eq!(params["api_key"], "test-key");
            let callback = Url::parse(&params["cb"]).unwrap();
            browser = Some(thread::spawn(move || {
                assert!(
                    browser_get(&callback, "/favicon.ico?token=wrong").starts_with("HTTP/1.1 404")
                );
                let target = format!("{}?token=browser%2Btoken", callback.path());
                assert!(browser_get(&callback, &target).starts_with("HTTP/1.1 200"));
            }));
            Ok(())
        },
    )
    .unwrap();
    browser.unwrap().join().unwrap();
    assert_eq!(account.name, "listener");
    account.now_playing(&played()).unwrap();
    account.scrobble(&played()).unwrap();

    let requests = server.join().unwrap();
    let expected = [
        (
            "auth.getToken",
            "api_keytest-keymethodauth.getTokentest-secret",
        ),
        (
            "auth.getSession",
            "api_keytest-keymethodauth.getSessiontokenbrowser+tokentest-secret",
        ),
        (
            "track.updateNowPlaying",
            "albumAlbum & oneapi_keytest-keyartistИсполнительduration40methodtrack.updateNowPlayingsktest-sessiontrackTrack + onetest-secret",
        ),
        (
            "track.scrobble",
            "album[0]Album & oneapi_keytest-keyartist[0]Исполнительduration[0]40methodtrack.scrobblesktest-sessiontimestamp[0]1234track[0]Track + onetest-secret",
        ),
    ];
    for (request, (method, signature)) in requests.iter().zip(expected) {
        assert_eq!(request.line, "POST /2.0/ HTTP/1.1");
        assert_eq!(
            request.headers["content-type"],
            "application/x-www-form-urlencoded"
        );
        let form = request.form();
        assert_eq!(form["method"], method);
        assert_eq!(form["api_key"], "test-key");
        assert_eq!(form["format"], "json");
        assert_eq!(
            form["api_sig"],
            format!("{:x}", Md5::digest(signature.as_bytes()))
        );
    }
    assert_eq!(requests[1].form()["token"], "browser+token");
    assert_eq!(requests[2].form()["track"], "Track + one");
    assert_eq!(requests[3].form()["timestamp[0]"], "1234");
}

#[test]
fn listenbrainz_validates_and_sends_only_credited_listens_with_a_timestamp() {
    let (endpoint, server) = server(&[
        (200, r#"{"valid":true,"user_name":"listener"}"#),
        (200, r#"{"status":"ok"}"#),
        (200, r#"{"status":"ok"}"#),
    ]);
    let account = listenbrainz::connect_at(&endpoint, " token ".into()).unwrap();
    assert_eq!(account.name, "listener");
    let mut play = played();
    account.now_playing(&play).unwrap();
    play.album = Some(String::new());
    account.scrobble(&play).unwrap();

    let requests = server.join().unwrap();
    for request in &requests {
        assert_eq!(request.headers["authorization"], "Token token");
    }
    assert_eq!(requests[0].line, "GET /1/validate-token HTTP/1.1");
    assert!(requests[0].body.is_empty());
    for request in &requests[1..] {
        assert_eq!(request.line, "POST /1/submit-listens HTTP/1.1");
        assert_eq!(
            request.headers["content-type"],
            "application/json; charset=utf-8"
        );
    }
    let now = requests[1].json();
    let single = requests[2].json();
    assert_eq!(now["listen_type"], "playing_now");
    assert!(now["payload"][0].get("listened_at").is_none());
    assert_eq!(
        now["payload"][0]["track_metadata"],
        json!({
            "artist_name": "Исполнитель",
            "track_name": "Track + one",
            "release_name": "Album & one",
            "additional_info": {
                "duration_ms": 40_500,
                "media_player": "Mediadeka",
                "submission_client": "Mediadeka",
            },
        })
    );
    assert_eq!(single["listen_type"], "single");
    assert_eq!(single["payload"][0]["listened_at"], 1234);
    assert!(
        single["payload"][0]["track_metadata"]
            .get("release_name")
            .is_none()
    );
}

#[test]
fn protocol_errors_do_not_expose_server_echoes() {
    let (last_endpoint, last_server) = server(&[
        (403, r#"{"error":26,"message":"echoed-secret"}"#),
        (
            200,
            r#"{"scrobbles":{"@attr":{"accepted":"0","ignored":"1"}}}"#,
        ),
    ]);
    let last_endpoint = format!("{last_endpoint}/2.0/");
    let result = audioscrobbler::connect_at(
        &last_endpoint,
        "http://127.0.0.1/approve",
        0,
        "test-key".into(),
        "test-secret".into(),
        &AtomicBool::new(false),
        |_| panic!("invalid key must be reported before opening approval"),
    );
    assert_eq!(result.err().as_deref(), Some("Last.fm отклонил ключ API."));
    let last = LastFmAccount::test_at(last_endpoint);
    assert_eq!(
        last.scrobble(&played()).unwrap_err(),
        "Last.fm не засчитал прослушивание."
    );
    last_server.join().unwrap();

    let (brainz_endpoint, brainz_server) = server(&[
        (200, r#"{"valid":false,"message":"echoed-token"}"#),
        (503, r#"{"error":"echoed-token"}"#),
    ]);
    let result = listenbrainz::connect_at(&brainz_endpoint, "test-token".into());
    assert_eq!(
        result.err().as_deref(),
        Some("ListenBrainz отклонил токен.")
    );
    let brainz = ListenBrainzAccount::test_at(brainz_endpoint);
    assert_eq!(
        brainz.now_playing(&played()).unwrap_err(),
        "ListenBrainz вернул HTTP 503."
    );
    brainz_server.join().unwrap();
}

#[test]
fn cancelling_last_fm_approval_drops_the_callback_listener() {
    let (endpoint, server) = server(&[(200, r#"{"token":"preflight-token"}"#)]);
    let cancelled = AtomicBool::new(false);
    let mut callback = None;
    let result = audioscrobbler::connect_at(
        &endpoint,
        "http://127.0.0.1/approve",
        0,
        "test-key".into(),
        "test-secret".into(),
        &cancelled,
        |authorize| {
            callback = Url::parse(authorize)
                .unwrap()
                .query_pairs()
                .find_map(|(key, value)| (key == "cb").then(|| Url::parse(&value).unwrap()));
            cancelled.store(true, Ordering::Relaxed);
            Ok(())
        },
    );
    assert_eq!(
        result.err().as_deref(),
        Some("Подключение Last.fm отменено.")
    );
    assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, callback.unwrap().port().unwrap())).is_ok());
    server.join().unwrap();
}

fn played() -> Play {
    Play {
        artist: "Исполнитель".into(),
        title: "Track + one".into(),
        album: Some("Album & one".into()),
        duration: Duration::from_millis(40_500),
        at: UNIX_EPOCH + Duration::from_secs(1234),
    }
}

fn browser_get(callback: &Url, target: &str) -> String {
    let port = callback.port().unwrap();
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    answer
}

pub(in crate::integrations) struct Request {
    line: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl Request {
    pub(in crate::integrations) fn form(&self) -> BTreeMap<String, String> {
        url::form_urlencoded::parse(&self.body)
            .into_owned()
            .collect()
    }

    pub(in crate::integrations) fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

fn server(replies: &'static [(u16, &'static str)]) -> (String, JoinHandle<Vec<Request>>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        replies.iter().map(|(status, response)| {
            let deadline = Instant::now() + Duration::from_secs(6);
            let mut connection = loop {
                match listener.accept() {
                    Ok((connection, _)) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("fixture did not receive its request: {error}"),
                }
            };
            connection.set_nonblocking(false).unwrap();
            connection.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let request = read_request(&mut connection);
            write!(connection, "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
            request
        }).collect()
    });
    (endpoint, worker)
}

pub(in crate::integrations) fn read_request(connection: &mut TcpStream) -> Request {
    let mut reader = BufReader::new(connection);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut headers = BTreeMap::new();
    loop {
        let mut header = String::new();
        assert_ne!(reader.read_line(&mut header).unwrap(), 0);
        if header == "\r\n" {
            break;
        }
        let (name, value) = header.split_once(':').unwrap();
        headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
    }
    let size = headers
        .get("content-length")
        .map_or(0, |length| length.parse().unwrap());
    let mut body = vec![0; size];
    reader.read_exact(&mut body).unwrap();
    Request {
        line: line.trim_end().to_owned(),
        headers,
        body,
    }
}
