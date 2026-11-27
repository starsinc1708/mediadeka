use std::{
    env,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    process::Command,
    thread,
};

use serde_json::json;

use super::*;

const LRC: &str = "[00:01.00]Timed line";
const PLAIN: &str = "Plain first line\nPlain second line";

#[test]
fn fetch_keeps_complete_partial_and_missing_formats_distinct() {
    const CHILD: &str = "MEDIADEKA_TEST_YANDEX_LYRICS_FIXTURE";
    const TEST: &str = "providers::yandex::lyrics::tests::fetch_keeps_complete_partial_and_missing_formats_distinct";
    if env::var_os(CHILD).is_none() {
        assert!(
            Command::new(env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture"])
                .env(CHILD, "1")
                .status()
                .unwrap()
                .success()
        );
        return;
    }

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    super::super::test_server::use_loopback(listener.local_addr().unwrap());
    let (attempts, observed) = std::sync::mpsc::channel();
    // Process isolation protects the global Yandex test endpoint.
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            let target = request.split_whitespace().nth(1).unwrap();
            let url = Url::parse(&format!("http://localhost{target}")).unwrap();
            let preference = url
                .query_pairs()
                .find(|(name, _)| name == "format")
                .unwrap()
                .1;
            let id = url
                .path()
                .strip_prefix("/tracks/")
                .unwrap()
                .strip_suffix("/lyrics")
                .unwrap();
            let (status, body) = response(id, &preference);
            attempts
                .send((id.to_owned(), preference.into_owned()))
                .unwrap();
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header == "\r\n" || header.is_empty() {
                    break;
                }
            }
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });

    let cancelled = AtomicBool::new(false);
    let load = |id| fetch("fixture-only", id, &cancelled);
    let complete = load("10:20").unwrap().unwrap();
    let timed = complete.preferred().clone();
    let plain = complete.alternate_plain().unwrap().clone();
    assert_eq!(timed.lines[0].time_ms, Some(1000));
    assert_eq!(timed.attribution.as_deref(), Some("Timed credits"));
    assert_eq!(plain.attribution.as_deref(), Some("Plain credits"));
    assert_eq!(plain.lines.len(), 2);
    assert!(plain.lines.iter().all(|line| line.time_ms.is_none()));
    assert_eq!(load("11:20").unwrap().unwrap(), timed.clone().into());
    assert_eq!(load("12:20").unwrap().unwrap(), plain.clone().into());
    assert_eq!(
        load("13:20").unwrap().unwrap(),
        LyricsVersions::from_source(plain, None)
    );
    assert!(load("14:20").unwrap().is_none());
    assert_eq!(load("15:20").err().unwrap().kind, ErrorKind::Network);
    assert_eq!(load("16:20").unwrap().unwrap(), timed.into());

    let expected: Vec<_> = (10..=16)
        .flat_map(|n| ["LRC", "TEXT"].map(move |format| (format!("{n}:20"), format.to_owned())))
        .collect();
    assert_eq!(observed.try_iter().collect::<Vec<_>>(), expected);
    let cancelled = AtomicBool::new(true);
    assert_eq!(
        fetch("fixture-only", "10:20", &cancelled)
            .err()
            .unwrap()
            .kind,
        ErrorKind::Cancelled
    );
    assert!(observed.try_recv().is_err());
}

fn response(id: &str, preference: &str) -> (u16, String) {
    let (text, credits) = match (id, preference) {
        ("11:20", "TEXT") | ("12:20", "LRC") | ("15:20", _) => return (503, String::new()),
        ("13:20" | "14:20", "LRC") => return (404, String::new()),
        ("14:20", "TEXT") => return (204, String::new()),
        ("16:20", "TEXT") | (_, "LRC") => (LRC, "Timed credits"),
        (_, "TEXT") => (PLAIN, "Plain credits"),
        _ => panic!("unexpected format"),
    };
    let body = json!({"result": {"lyrics": text, "major": {"prettyName": credits}}});
    (200, body.to_string())
}
