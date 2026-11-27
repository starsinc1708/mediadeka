use std::{
    io::{self, BufRead, BufReader, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::{FIRST, NEXT, STATION_NAME};

#[derive(Default)]
pub(super) struct Requests {
    pub(super) starts: AtomicUsize,
    pub(super) first_replied: AtomicBool,
    pub(super) release_first: AtomicBool,
    pub(super) feedback: AtomicUsize,
    pub(super) refills: AtomicUsize,
    pub(super) media: AtomicUsize,
    pub(super) stop: AtomicBool,
}

pub(super) fn serve(listener: TcpListener, audio: &[u8], requests: &Requests) {
    let address = listener.local_addr().unwrap();
    while !requests.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => answer(stream, address, audio, requests),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(4))
            }
            Err(error) => panic!("{error}"),
        }
    }
}

fn answer(mut stream: TcpStream, address: SocketAddr, audio: &[u8], requests: &Requests) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut input = BufReader::new(&stream);
    let mut line = String::new();
    if input.read_line(&mut line).unwrap_or_default() == 0 {
        return;
    }
    let path = line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default()
        .to_owned();
    loop {
        line.clear();
        if input.read_line(&mut line).unwrap_or_default() == 0 || line.trim().is_empty() {
            break;
        }
    }
    let mut first = false;
    let body = match path.as_str() {
        "/rotor/session/new" => {
            first = requests.starts.fetch_add(1, Ordering::Relaxed) == 0;
            let deadline = Instant::now() + Duration::from_secs(10);
            while first
                && !requests.release_first.load(Ordering::Relaxed)
                && Instant::now() < deadline
            {
                thread::sleep(Duration::from_millis(4));
            }
            batch(FIRST, STATION_NAME, "initial")
        }
        "/rotor/session/acceptance/feedback" => {
            requests.feedback.fetch_add(1, Ordering::Relaxed);
            json!({"result":{}})
        }
        "/rotor/session/acceptance/tracks" => {
            requests.refills.fetch_add(1, Ordering::Relaxed);
            batch(NEXT, "Станция · продолжение", "next")
        }
        "/account/status" => json!({"result":{"plus":{"hasPlus":true}}}),
        "/get-file-info" => json!({"result":{"downloadInfo":{"codec":"flac","transport":"raw",
            "url":format!("http://{address}/test.flac")}}}),
        "/test.flac" => {
            requests.media.fetch_add(1, Ordering::Relaxed);
            respond(&mut stream, "200 OK", "audio/flac", audio);
            return;
        }
        _ => {
            respond(&mut stream, "404 Not Found", "text/plain", b"");
            return;
        }
    };
    respond(
        &mut stream,
        "200 OK",
        "application/json",
        body.to_string().as_bytes(),
    );
    if first {
        requests.first_replied.store(true, Ordering::Relaxed);
    }
}

fn batch(id: usize, title: &str, batch_id: &str) -> Value {
    json!({"result":{"radioSessionId":"acceptance","batchId":batch_id,"sequence":[
      {"type":"track","track":{"id":id,"title":title,"available":true,"durationMs":30_000}}
    ]}})
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, bytes: &[u8]) {
    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len())
        .and_then(|_| stream.write_all(bytes));
}
