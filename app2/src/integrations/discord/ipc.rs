
use std::{io, thread, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    net::windows::named_pipe::{ClientOptions, NamedPipeClient},
    runtime::Builder,
    sync::watch,
    time::timeout,
};

use super::Activity;

const RETRY_DELAY: Duration = Duration::from_secs(3);
const SWITCH_DEBOUNCE: Duration = Duration::from_secs(1);
const ROUND_TRIP_TIMEOUT: Duration = Duration::from_secs(1);
const PIPE_SLOTS: u8 = 10;
const MAX_FRAME_BYTES: u32 = 1 << 20;
const HANDSHAKE: u32 = 0;
const FRAME: u32 = 1;
const CLOSE: u32 = 2;
const PING: u32 = 3;
const PONG: u32 = 4;

pub(super) fn spawn(wanted: Option<Activity>) -> Option<watch::Sender<Option<Activity>>> {
    let paths = (0..PIPE_SLOTS)
        .map(|slot| format!(r"\\?\pipe\discord-ipc-{slot}"))
        .collect();
    spawn_on(wanted, paths)
}

pub(super) fn spawn_on(
    wanted: Option<Activity>,
    paths: Vec<String>,
) -> Option<watch::Sender<Option<Activity>>> {
    let (sender, receiver) = watch::channel(wanted);
    thread::Builder::new()
        .name("discord-presence".into())
        .spawn(move || {
            if let Ok(runtime) = Builder::new_current_thread().enable_all().build() {
                runtime.block_on(run(receiver, paths));
            }
        })
        .ok()
        .map(|_| sender)
}

async fn run(mut receiver: watch::Receiver<Option<Activity>>, paths: Vec<String>) {
    let mut connection = None;
    let mut shown = None;
    while let Some(wanted) = next_presence(&mut receiver, &shown).await {
        let Some(activity) = wanted else {
            clear(&mut connection).await;
            shown = None;
            continue;
        };
        match send(&paths, &mut connection, &activity).await {
            // A refused activity stays marked as offered until the desired activity changes.
            Ok(_) => shown = Some(activity),
            Err(_) => {
                shown = None;
                if matches!(timeout(RETRY_DELAY, receiver.changed()).await, Ok(Err(_))) {
                    break;
                }
            }
        }
    }
    clear(&mut connection).await;
}

/// Debounces changes between tracks, while clearing presence remains immediate.
async fn next_presence(
    receiver: &mut watch::Receiver<Option<Activity>>,
    shown: &Option<Activity>,
) -> Option<Option<Activity>> {
    loop {
        let wanted = receiver.borrow_and_update().clone();
        if &wanted == shown {
            receiver.changed().await.ok()?;
            continue;
        }
        if wanted.is_none() {
            return Some(None);
        }
        match timeout(SWITCH_DEBOUNCE, receiver.changed()).await {
            Ok(changed) => changed.ok()?,
            Err(_) => return Some(wanted),
        }
    }
}

async fn send(
    paths: &[String],
    connection: &mut Option<Connection>,
    activity: &Activity,
) -> io::Result<Reply> {
    if let Some(mut client) = connection.take() {
        if client.application_id != activity.application_id {
            let _ = client.offer(None).await;
        } else if let Ok(reply) = client.offer(Some(activity)).await {
            *connection = Some(client);
            return Ok(reply);
        }
    }
    let mut client = Connection::connect(paths, &activity.application_id).await?;
    let reply = client.offer(Some(activity)).await?;
    *connection = Some(client);
    Ok(reply)
}

async fn clear(connection: &mut Option<Connection>) {
    if let Some(mut client) = connection.take() {
        let _ = client.offer(None).await;
    }
}

enum Reply {
    Accepted,
    Refused,
}

struct Connection {
    application_id: String,
    pipe: NamedPipeClient,
    sequence: u64,
}

impl Connection {
    async fn connect(paths: &[String], application_id: &str) -> io::Result<Self> {
        for path in paths {
            let Ok(mut pipe) = ClientOptions::new().open(path) else {
                continue;
            };
            let ready = timeout(ROUND_TRIP_TIMEOUT, async {
                write_json(
                    &mut pipe,
                    HANDSHAKE,
                    &json!({"v": 1, "client_id": application_id}),
                )
                .await?;
                reply(&mut pipe, |data| {
                    matches!(data["evt"].as_str(), Some("READY" | "ERROR"))
                })
                .await
            })
            .await;
            if let Ok(Ok(answer)) = ready
                && answer["evt"] == "READY"
            {
                return Ok(Self {
                    application_id: application_id.to_owned(),
                    pipe,
                    sequence: 0,
                });
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "Discord IPC is unavailable",
        ))
    }

    async fn offer(&mut self, activity: Option<&Activity>) -> io::Result<Reply> {
        self.sequence = self.sequence.wrapping_add(1);
        let nonce = self.sequence.to_string();
        let message = json!({
            "cmd": "SET_ACTIVITY",
            "args": {"pid": std::process::id(), "activity": activity},
            "nonce": nonce,
        });
        let answer = timeout(ROUND_TRIP_TIMEOUT, async {
            write_json(&mut self.pipe, FRAME, &message).await?;
            reply(&mut self.pipe, |data| data["nonce"] == nonce).await
        })
        .await??;
        Ok(if answer["evt"] == "ERROR" {
            Reply::Refused
        } else {
            Reply::Accepted
        })
    }
}

async fn reply(
    pipe: &mut (impl AsyncRead + AsyncWrite + Unpin),
    matches: impl Fn(&Value) -> bool,
) -> io::Result<Value> {
    loop {
        let (op, body) = read_frame(pipe).await?;
        match op {
            PING => write_frame(pipe, PONG, &body).await?,
            CLOSE => return Err(io::ErrorKind::ConnectionReset.into()),
            FRAME => {
                let json = serde_json::from_slice(&body)?;
                if matches(&json) {
                    return Ok(json);
                }
            }
            _ => {}
        }
    }
}

pub(super) async fn write_json(
    pipe: &mut (impl AsyncWrite + Unpin),
    opcode: u32,
    value: &Value,
) -> io::Result<()> {
    write_frame(pipe, opcode, &serde_json::to_vec(value)?).await
}

async fn write_frame(
    pipe: &mut (impl AsyncWrite + Unpin),
    opcode: u32,
    body: &[u8],
) -> io::Result<()> {
    let length = u32::try_from(body.len()).map_err(io::Error::other)?;
    let mut header = [0; 8];
    header[..4].copy_from_slice(&opcode.to_le_bytes());
    header[4..].copy_from_slice(&length.to_le_bytes());
    pipe.write_all(&header).await?;
    pipe.write_all(body).await
}

pub(super) async fn read_frame(pipe: &mut (impl AsyncRead + Unpin)) -> io::Result<(u32, Vec<u8>)> {
    let mut header = [0; 8];
    pipe.read_exact(&mut header).await?;
    let [o0, o1, o2, o3, l0, l1, l2, l3] = header;
    let opcode = u32::from_le_bytes([o0, o1, o2, o3]);
    let length = u32::from_le_bytes([l0, l1, l2, l3]);
    if length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Discord IPC frame is too large",
        ));
    }
    let mut body = vec![0; length as usize];
    pipe.read_exact(&mut body).await?;
    Ok((opcode, body))
}
