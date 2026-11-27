mod acceptance;

use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{Barrier, Mutex},
};

use sha1::{Digest, Sha1};

use crate::providers::downloads::DownloadCandidate;

use super::*;

fn request(stream: &TcpStream) -> Vec<String> {
    let mut request = BufReader::new(stream);
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        request.read_line(&mut line).unwrap();
        if line == "\r\n" || line.is_empty() {
            break;
        }
        headers.push(line.trim().to_owned());
    }
    headers
}

#[test]
fn resumes_partial_transfer_in_a_new_attempt_with_a_verified_range() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut candidate = DownloadCandidate::new(
        url::Url::parse(&format!(
            "http://{}/original",
            listener.local_addr().unwrap()
        ))
        .unwrap(),
    );
    let bytes = vec![b'a'; 100_000];
    candidate.bytes = Some(bytes.len() as u64);
    candidate.sha1 = Some(Sha1::digest(&bytes).into());
    let directory = tempfile::tempdir().unwrap();
    let recorded = AtomicBool::new(false);
    let split = 27_000;
    thread::scope(|scope| {
        scope.spawn(|| {
            let (mut stream, _) = listener.accept().unwrap();
            assert!(!request(&stream).iter().any(|header| header.to_lowercase().starts_with("range:")));
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"file-v1\"\r\nConnection: close\r\n\r\n", bytes.len()).unwrap();
            stream.write_all(&bytes[..split]).unwrap();
            drop(stream);
            let (mut stream, _) = listener.accept().unwrap();
            let request = request(&stream).join("\n").to_lowercase();
            assert!(request.contains(&format!("range: bytes={split}-")));
            assert!(request.contains("if-range: \"file-v1\""));
            recorded.store(true, Ordering::Release);
            write!(stream, "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {split}-{}/{}\r\nETag: \"file-v1\"\r\nConnection: close\r\n\r\n", bytes.len()-split, bytes.len()-1, bytes.len()).unwrap();
            stream.write_all(&bytes[split..]).unwrap();
        });
        let first = transfer::fetch(
            directory.path(),
            "fixture",
            &candidate,
            &AtomicBool::new(false),
        );
        assert!(first.is_err());
        let downloaded = transfer::fetch(
            directory.path(),
            "fixture",
            &candidate,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(std::fs::read(&downloaded).unwrap(), bytes);
    });
    assert!(recorded.load(Ordering::Acquire));
}

fn result(directory: &Path, codec: &str, bitrate: u32) -> Verified {
    let path = Arc::new(
        tempfile::NamedTempFile::new_in(directory)
            .unwrap()
            .into_temp_path(),
    );
    Verified {
        path,
        quality: AudioQuality::new(codec.into(), Some(bitrate), 48_000),
        declared: Vec::new(),
    }
}

#[test]
fn first_verified_source_starts_and_the_better_one_replaces_its_cache_entry() {
    let directory = tempfile::tempdir().unwrap();
    let parallel = Barrier::new(2);
    let started = AtomicBool::new(false);
    let cached = Mutex::new(Vec::new());
    let (played, errors) = race(
        vec![
            (ProviderId::SoundCloud, false),
            (ProviderId::Jellyfin, true),
        ],
        &AtomicBool::new(false),
        IMPROVEMENT_WINDOW,
        |lossless, stop| {
            parallel.wait();
            if lossless {
                while !started.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
                    thread::yield_now();
                }
                Ok(result(directory.path(), "flac", 700_000))
            } else {
                Ok(result(directory.path(), "mp3", 320_000))
            }
        },
        |provider, file| {
            assert_eq!(provider, ProviderId::SoundCloud);
            assert_eq!(file.quality.codec, "mp3");
            started.store(true, Ordering::Release);
            true
        },
        |file| cached.lock().unwrap().push(file.quality.codec.clone()),
    );
    assert!(played && errors.is_empty());
    assert_eq!(*cached.lock().unwrap(), ["mp3", "flac"]);
}

#[test]
fn replacing_the_track_stops_competing_transfers_and_prevents_cache_updates() {
    let directory = tempfile::tempdir().unwrap();
    let cancelled = AtomicBool::new(false);
    let stopped = AtomicBool::new(false);
    let (played, _) = race(
        vec![
            (ProviderId::SoundCloud, false),
            (ProviderId::Jellyfin, true),
        ],
        &cancelled,
        IMPROVEMENT_WINDOW,
        |slow, stop| {
            if slow {
                while !stop.load(Ordering::Acquire) {
                    thread::yield_now();
                }
                stopped.store(true, Ordering::Release);
                Err("cancelled".into())
            } else {
                Ok(result(directory.path(), "mp3", 320_000))
            }
        },
        |_, _| {
            cancelled.store(true, Ordering::Release);
            true
        },
        |_| panic!("cancelled audio must not update the cache"),
    );
    assert!(played && stopped.load(Ordering::Acquire));
}

#[test]
fn the_improvement_window_ends_a_slow_source_while_playback_keeps_its_first_file() {
    let directory = tempfile::tempdir().unwrap();
    let slow_stopped = AtomicBool::new(false);
    let cached = Mutex::new(Vec::new());
    let (played, _) = race(
        vec![
            (ProviderId::SoundCloud, false),
            (ProviderId::Jellyfin, true),
        ],
        &AtomicBool::new(false),
        Duration::from_millis(100),
        |slow, stop| {
            if slow {
                while !stop.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(10));
                }
                slow_stopped.store(true, Ordering::Release);
                Err("cancelled".into())
            } else {
                Ok(result(directory.path(), "mp3", 320_000))
            }
        },
        |provider, _| provider == ProviderId::SoundCloud,
        |file| cached.lock().unwrap().push(file.quality.codec.clone()),
    );
    assert!(played && slow_stopped.load(Ordering::Acquire));
    assert_eq!(*cached.lock().unwrap(), ["mp3"]);
}
