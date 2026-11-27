use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, Write},
    os::windows::fs::OpenOptionsExt,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tempfile::TempPath;
use ureq::{Agent, Body, http::Response};

use crate::providers::downloads::DownloadCandidate;

const MAX_AUDIO_BYTES: u64 = 128 * 1024 * 1024;
const CHECKPOINT_BYTES: u64 = 1024 * 1024;
const STRONG_DATE_AGE: Duration = Duration::from_secs(60);
const RETRY_MESSAGE: &str = "Передача оригинала прервана. Повтор продолжит доступную загрузку.";

#[derive(Deserialize, Serialize)]
struct Checkpoint {
    resource: String,
    validator: Validator,
    total: u64,
    received: u64,
    prefix: String,
}

#[derive(Deserialize, Serialize)]
enum Validator {
    Etag(String),
    Modified(String),
}

impl Validator {
    fn value(&self) -> &str {
        match self {
            Self::Etag(value) | Self::Modified(value) => value,
        }
    }

    fn header(&self) -> &'static str {
        match self {
            Self::Etag(_) => "etag",
            Self::Modified(_) => "last-modified",
        }
    }

    fn valid(&self) -> bool {
        match self {
            Self::Etag(value) => strong_etag(value),
            Self::Modified(value) => httpdate::parse_http_date(value).is_ok(),
        }
    }
}

pub(super) fn fetch(
    directory: &Path,
    identity: &str,
    candidate: &DownloadCandidate,
    cancelled: &AtomicBool,
) -> Result<TempPath, String> {
    let key = format!("{:x}", Sha256::digest(identity));
    let part = directory.join(format!("{key}.part"));
    let state_file = directory.join(format!("{key}.json"));
    let _claim = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(directory.join(format!("{key}.lock")))
        .map_err(|_| "Оригинал уже загружается в этом профиле.")?;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(&part)
        .map_err(|_| "Не удалось открыть незавершённую загрузку.")?;
    let resource = resource(&candidate.url);
    let checkpoint = checkpoint(&state_file, &mut file, &resource, candidate.bytes);
    let offset = checkpoint.as_ref().map_or(0, |(saved, _)| saved.received);
    let agent: Agent = Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_recv_response(Some(Duration::from_secs(6)))
        .timeout_recv_body(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut request = agent
        .get(candidate.url.as_str())
        .header("accept-encoding", "identity");
    for (name, value) in &candidate.headers {
        request = request.header(name, value);
    }
    if let Some((saved, _)) = &checkpoint {
        request = request
            .header("range", format!("bytes={offset}-"))
            .header("if-range", saved.validator.value());
    }
    check_cancelled(cancelled)?;
    let mut response = request.call().map_err(|_| RETRY_MESSAGE.to_owned())?;
    if header(&response, "content-encoding").is_some_and(|name| name != "identity") {
        return Err("Передача оригинала использовала недопустимое кодирование.".into());
    }
    let length = header(&response, "content-length")
        .map(str::parse::<u64>)
        .transpose()
        .map_err(|_| "Некорректный размер оригинала.")?;
    let (mut state, mut digest, start, total) = match (response.status().as_u16(), checkpoint) {
        (206, Some((state, digest))) if valid_range(&response, &state, length) => {
            let total = state.total;
            (Some(state), digest, offset, Some(total))
        }
        (200, _) => {
            let _ = fs::remove_file(&state_file);
            let total = candidate.bytes.or(length);
            if candidate
                .bytes
                .zip(length)
                .is_some_and(|(expected, served)| expected != served)
            {
                return Err("Размер оригинала не совпадает с карточкой.".into());
            }
            let state = total
                .zip(validator(&response))
                .map(|(total, validator)| Checkpoint {
                    resource,
                    validator,
                    total,
                    received: 0,
                    prefix: String::new(),
                });
            (state, Sha256::new(), 0, total)
        }
        (206 | 416, _) => {
            let _ = fs::remove_file(&state_file);
            return Err("Сервер вернул неверный диапазон оригинала.".into());
        }
        (status, _) => return Err(format!("Сервер оригинала вернул HTTP {status}.")),
    };
    if total.is_some_and(|len| len == 0 || len > MAX_AUDIO_BYTES) {
        return Err("Размер оригинала выходит за пределы загрузки.".into());
    }
    file.set_len(start)
        .map_err(|_| "Не удалось подготовить файл загрузки.")?;
    file.seek(std::io::SeekFrom::Start(start))
        .map_err(|_| "Не удалось продолжить загрузку.")?;

    let mut body = response.body_mut().as_reader();
    let mut received = start;
    let mut buffer = vec![0_u8; 64 * 1024];
    let transfer = loop {
        if cancelled.load(Ordering::Acquire) {
            break Err("Загрузка звука отменена.");
        }
        let count = match body.read(&mut buffer) {
            Ok(0) => break Ok(()),
            Ok(count) => count,
            Err(_) => break Err(RETRY_MESSAGE),
        };
        if cancelled.load(Ordering::Acquire) {
            break Err("Загрузка звука отменена.");
        }
        received += count as u64;
        if received > total.unwrap_or(MAX_AUDIO_BYTES) {
            return Err("Сервер превысил допустимый размер оригинала.".into());
        }
        file.write_all(&buffer[..count])
            .map_err(|_| "Не удалось записать оригинал.")?;
        digest.update(&buffer[..count]);
        if state.as_ref().is_some_and(|saved| {
            saved.received == 0 || received - saved.received >= CHECKPOINT_BYTES
        }) {
            write_checkpoint(directory, &state_file, &file, &mut state, received, &digest)?;
        }
    };
    if transfer.is_err() || total.is_some_and(|len| received != len) {
        write_checkpoint(directory, &state_file, &file, &mut state, received, &digest)?;
        return Err(transfer.err().unwrap_or(RETRY_MESSAGE).to_owned());
    }
    check_cancelled(cancelled)?;
    if received == 0 {
        return Err("Сервер вернул пустой оригинал.".into());
    }
    if let Some(expected) = candidate.sha1 {
        file.rewind()
            .map_err(|_| "Не удалось прочитать оригинал для проверки.")?;
        let mut hash = Sha1::new();
        std::io::copy(&mut file, &mut hash).map_err(|_| "Не удалось проверить хэш оригинала.")?;
        if <[u8; 20]>::from(hash.finalize()) != expected {
            file.set_len(0).map_err(|_| "Хэш оригинала не совпал.")?;
            let _ = fs::remove_file(&state_file);
            return Err("Хэш оригинала не совпал.".into());
        }
    }
    file.sync_all()
        .map_err(|_| "Не удалось завершить запись оригинала.")?;
    drop(file);
    let finished = tempfile::Builder::new()
        .prefix("ready-")
        .tempfile_in(directory)
        .map_err(|_| "Не удалось подготовить проверку оригинала.")?
        .into_temp_path();
    fs::rename(&part, &finished).map_err(|_| "Не удалось передать оригинал на проверку.")?;
    let _ = fs::remove_file(&state_file);
    Ok(finished)
}

fn checkpoint(
    path: &Path,
    file: &mut File,
    resource: &str,
    bytes: Option<u64>,
) -> Option<(Checkpoint, Sha256)> {
    let input = File::open(path).ok()?;
    if input.metadata().ok()?.len() > 4096 {
        return None;
    }
    let state: Checkpoint = serde_json::from_reader(input).ok()?;
    if state.resource != resource
        || !state.validator.valid()
        || state.received == 0
        || state.received >= state.total
        || state.total > MAX_AUDIO_BYTES
        || bytes.is_some_and(|bytes| bytes != state.total)
    {
        return None;
    }
    file.rewind().ok()?;
    let mut prefix = file.take(state.received);
    let mut digest = Sha256::new();
    if std::io::copy(&mut prefix, &mut digest).ok()? != state.received
        || format!("{:x}", digest.clone().finalize()) != state.prefix
    {
        return None;
    }
    Some((state, digest))
}

fn write_checkpoint(
    directory: &Path,
    path: &Path,
    file: &File,
    state: &mut Option<Checkpoint>,
    received: u64,
    digest: &Sha256,
) -> Result<(), String> {
    let Some(state) = state
        .as_mut()
        .filter(|state| state.received < received && received < state.total)
    else {
        return Ok(());
    };
    let mut save = || -> std::io::Result<()> {
        file.sync_data()?;
        state.received = received;
        state.prefix = format!("{:x}", digest.clone().finalize());
        let mut atomic = tempfile::NamedTempFile::new_in(directory)?;
        serde_json::to_writer(&mut atomic, state)?;
        atomic.as_file().sync_all()?;
        atomic.persist(path).map_err(|error| error.error)?;
        Ok(())
    };
    save().map_err(|_| "Не удалось сохранить состояние незавершённой загрузки.".into())
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("Загрузка звука отменена.".into());
    }
    Ok(())
}

fn strong_etag(value: &str) -> bool {
    (2..=512).contains(&value.len())
        && value.starts_with('"')
        && value.ends_with('"')
        && value.as_bytes()[1..value.len() - 1]
            .iter()
            .all(|byte| *byte == b'!' || (0x23..=0x7e).contains(byte))
}

fn validator(response: &Response<Body>) -> Option<Validator> {
    if let Some(etag) = header(response, "etag") {
        return strong_etag(etag).then(|| Validator::Etag(etag.to_owned()));
    }
    let modified = header(response, "last-modified")?;
    let date = httpdate::parse_http_date(header(response, "date")?).ok()?;
    let change = httpdate::parse_http_date(modified).ok()?;
    (date.duration_since(change).ok()? >= STRONG_DATE_AGE)
        .then(|| Validator::Modified(modified.to_owned()))
}

fn header<'a>(response: &'a Response<Body>, name: &str) -> Option<&'a str> {
    response.headers().get(name)?.to_str().ok()
}

fn valid_range(response: &Response<Body>, state: &Checkpoint, length: Option<u64>) -> bool {
    header(response, state.validator.header()) == Some(state.validator.value())
        && header(response, "content-range")
            == Some(
                format!(
                    "bytes {}-{}/{}",
                    state.received,
                    state.total - 1,
                    state.total
                )
                .as_str(),
            )
        && length.is_none_or(|length| length == state.total - state.received)
}

fn resource(url: &url::Url) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!(
            "{}{}",
            url.origin().ascii_serialization(),
            url.path()
        ))
    )
}
