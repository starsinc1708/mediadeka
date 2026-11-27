use std::{
    io::{ErrorKind, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use url::Url;

const WAIT: Duration = Duration::from_secs(300);
const POLL: Duration = Duration::from_millis(50);
const CONNECTION_TIMEOUT: Duration = Duration::from_millis(700);
const REQUEST_LIMIT: u64 = 8192;

pub(super) struct Approval {
    listener: TcpListener,
    address: SocketAddr,
    url: Url,
}

impl Approval {
    pub(super) fn bind(port: u16) -> Result<Self, String> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .map_err(|_| "Порт возврата Last.fm занят или недоступен.")?;
        listener
            .set_nonblocking(true)
            .map_err(|_| "Порт возврата Last.fm недоступен.")?;
        let address = listener
            .local_addr()
            .map_err(|_| "Порт возврата Last.fm недоступен.")?;

        // Keep unrelated loopback requests out of this one approval attempt.
        let mut nonce = [0; 16];
        getrandom::fill(&mut nonce).map_err(|_| "Не удалось начать подключение Last.fm.")?;
        let path = URL_SAFE_NO_PAD.encode(nonce);
        let url = Url::parse(&format!("http://{address}/scrobble/{path}"))
            .map_err(|_| "Не удалось сформировать адрес возврата Last.fm.")?;
        Ok(Self {
            listener,
            address,
            url,
        })
    }

    pub(super) fn url(&self) -> &Url {
        &self.url
    }

    /// Ignore browser prefetches and requests outside the current approval path.
    pub(super) fn token(self, cancelled: &AtomicBool) -> Result<String, String> {
        let deadline = Instant::now() + WAIT;
        let host = self.address.to_string();
        loop {
            check_cancelled(cancelled)?;
            if Instant::now() >= deadline {
                return Err("Время ожидания подтверждения Last.fm истекло.".into());
            }
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    if let Some(token) = read_callback(&mut stream, &self.url, &host, deadline) {
                        return Ok(token);
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => thread::sleep(POLL),
                Err(_) => return Err("Не удалось получить ответ браузера Last.fm.".into()),
            }
        }
    }
}

pub(super) fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Подключение Last.fm отменено.".into());
    }
    Ok(())
}

fn read_callback(
    stream: &mut TcpStream,
    callback: &Url,
    host: &str,
    deadline: Instant,
) -> Option<String> {
    stream.set_nonblocking(false).ok()?;
    stream.set_write_timeout(Some(CONNECTION_TIMEOUT)).ok()?;
    let deadline = deadline.min(Instant::now() + CONNECTION_TIMEOUT);
    let mut bytes = Vec::new();
    let mut request = stream.take(REQUEST_LIMIT);
    let mut chunk = [0; 256];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        request.get_mut().set_read_timeout(Some(remaining)).ok()?;
        let count = request.read(&mut chunk).ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let request = std::str::from_utf8(&bytes).ok()?;
    let token = parse_callback(request, callback, host);
    let (status, text) = if token.is_some() {
        (
            "200 OK",
            "Вернитесь в Mediadeka для завершения подключения Last.fm.",
        )
    } else {
        ("404 Not Found", "Адрес подтверждения Last.fm не найден.")
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    token
}

fn parse_callback(request: &str, callback: &Url, host: &str) -> Option<String> {
    let mut lines = request.lines();
    let (target, _) = lines.next()?.strip_prefix("GET ")?.split_once(' ')?;
    if !target.starts_with('/') || target.starts_with("//") || target.contains('\\') {
        return None;
    }
    let received_host = lines.take_while(|line| !line.is_empty()).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case("host").then(|| value.trim())
    });
    if received_host != Some(host) {
        return None;
    }
    let url = callback.join(target).ok()?;
    if url.path() != callback.path() || url.origin() != callback.origin() {
        return None;
    }
    url.query_pairs()
        .find_map(|(key, value)| (key == "token" && !value.is_empty()).then(|| value.into_owned()))
}
