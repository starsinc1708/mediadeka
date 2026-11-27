use std::{
    io::{BufRead, BufReader, Read},
    process::{Child, ExitStatus, Stdio},
    sync::{
        atomic::AtomicBool,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use super::{PairProgress, Pairing, check_cancelled};
use crate::soloist::{
    daemon::{self, Job},
    install::Installed,
};

const PAIR_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const POLL: Duration = Duration::from_millis(50);
const LOG_LINE_BYTES: u64 = 16 * 1024;
const LISTENING: &str = "Started ZeroConf service on port ";
const REFUSED: &str = "login failed";
const KEY_REJECTED: &str = "Spotify отклонил ключ Soloist. Проверьте ключ и повторите сопряжение.";
const PROCESS_ERROR: &str = "Не удалось запустить сопряжение Spotify Soloist.";
const EXPIRED_CODE: i32 = 10;

enum Event {
    Listening(u16),
    Rejected,
}

pub(super) fn run(
    installed: &Installed,
    pairing: &Pairing,
    key: &str,
    cancelled: &AtomicBool,
    progress: &impl Fn(PairProgress),
) -> Result<(), String> {
    check_cancelled(cancelled)?;
    let mut child = daemon::command(installed, &pairing.data, &pairing.cache, key)
        .args(["--pair", "--verbose"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| PROCESS_ERROR)?;
    let job = match Job::attach(&child) {
        Ok(job) => job,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PROCESS_ERROR.into());
        }
    };

    let (send, events) = mpsc::channel();
    let mut readers = Vec::with_capacity(2);
    let started = (|| {
        let stdout = child.stdout.take().ok_or(PROCESS_ERROR)?;
        readers.push(follow(stdout, send.clone())?);
        let stderr = child.stderr.take().ok_or(PROCESS_ERROR)?;
        readers.push(follow(stderr, send)?);
        Ok(())
    })();
    let mut listening = false;
    let status =
        started.and_then(|()| watch(&mut child, &events, cancelled, progress, &mut listening));

    // Closing the process group also closes any copies of our pipes held by child helpers.
    drop(job);
    let _ = child.wait();
    for reader in readers {
        let _ = reader.join();
    }
    check_cancelled(cancelled)?;
    let status = status?;
    drain(&events, progress, &mut listening)?;
    if status.success() {
        Ok(())
    } else if status.code() == Some(EXPIRED_CODE) {
        Err("Установленная версия Spotify Soloist устарела.".into())
    } else {
        Err(format!(
            "Процесс сопряжения Spotify Soloist завершился с ошибкой ({status})."
        ))
    }
}

fn watch(
    child: &mut Child,
    events: &Receiver<Event>,
    cancelled: &AtomicBool,
    progress: &impl Fn(PairProgress),
    listening: &mut bool,
) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + PAIR_TIMEOUT;
    loop {
        check_cancelled(cancelled)?;
        drain(events, progress, listening)?;
        if let Some(status) = child.try_wait().map_err(|_| PROCESS_ERROR)? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(
                "Устройство Mediadeka Soloist не выбрали в Spotify за отведённое время.".into(),
            );
        }
        thread::sleep(POLL);
    }
}

fn drain(
    events: &Receiver<Event>,
    progress: &impl Fn(PairProgress),
    listening: &mut bool,
) -> Result<(), String> {
    for event in events.try_iter() {
        match event {
            Event::Rejected => return Err(KEY_REJECTED.into()),
            Event::Listening(port) if !*listening => {
                *listening = true;
                progress(PairProgress::Waiting { port });
            }
            Event::Listening(_) => {}
        }
    }
    Ok(())
}

fn follow(
    reader: impl Read + Send + 'static,
    events: Sender<Event>,
) -> Result<JoinHandle<()>, String> {
    thread::Builder::new()
        .name("soloist-pairing-output".into())
        .spawn(move || {
            let mut reader = BufReader::new(reader);
            let mut bytes = Vec::new();
            let mut listening = false;
            loop {
                bytes.clear();
                match (&mut reader)
                    .take(LOG_LINE_BYTES + 1)
                    .read_until(b'\n', &mut bytes)
                {
                    Ok(0) | Err(_) => break,
                    Ok(read) if read as u64 > LOG_LINE_BYTES => {
                        if !bytes.ends_with(b"\n") && reader.skip_until(b'\n').is_err() {
                            break;
                        }
                        continue;
                    }
                    Ok(_) => {}
                }
                // Inspect only the two required protocol markers. Raw output can contain the key.
                let line = String::from_utf8_lossy(&bytes);
                if line.contains(REFUSED) {
                    let _ = events.send(Event::Rejected);
                    break;
                }
                if !listening && let Some(port) = port_in(&line) {
                    if events.send(Event::Listening(port)).is_err() {
                        break;
                    }
                    listening = true;
                }
            }
        })
        .map_err(|_| PROCESS_ERROR.into())
}

fn port_in(line: &str) -> Option<u16> {
    let (_, rest) = line.split_once(LISTENING)?;
    rest.split_whitespace()
        .next()?
        .parse()
        .ok()
        .filter(|port| *port != 0)
}

#[cfg(test)]
#[test]
fn keeps_only_a_real_listener_port() {
    assert_eq!(
        port_in("INFO Started ZeroConf service on port 60123 [f: example]"),
        Some(60123)
    );
    assert_eq!(port_in("Started ZeroConf service on port 0"), None);
    assert_eq!(port_in("Started ZeroConf service on port -1"), None);
    assert_eq!(port_in("Not a listening message"), None);
}
