use std::{
    io::ErrorKind,
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};
use tungstenite::{
    Error as WebSocketError, Message, WebSocket, client::client_with_config,
    protocol::WebSocketConfig,
};

use super::{COMMAND_ERROR, CONNECT_TIMEOUT, DISCONNECTED, POLL_INTERVAL, WIRE_ERROR};

const REFRESH_INTERVAL: Duration = Duration::from_millis(180);
const STATE_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum Status {
    #[default]
    Idle,
    Buffering,
    Playing,
    Paused,
}

impl Status {
    fn parse(value: &Value) -> Self {
        match value.as_str() {
            Some("buffering") => Self::Buffering,
            Some("playing") => Self::Playing,
            Some("paused") => Self::Paused,
            _ => Self::Idle,
        }
    }

    pub(super) fn advancing(self) -> bool {
        matches!(self, Self::Buffering | Self::Playing)
    }
}

#[derive(Default)]
pub(super) struct Position {
    pub(super) milliseconds: u64,
    unix_ms: u64,
    pub(super) speed: f64,
}

impl Position {
    fn parse(value: &Value) -> Self {
        Self {
            milliseconds: value["position_ms"].as_u64().unwrap_or_default(),
            unix_ms: value["timestamp_ms"].as_u64().unwrap_or_default(),
            speed: value["speed"].as_f64().unwrap_or_default().clamp(0.0, 4.0),
        }
    }

    pub(super) fn seconds(&self) -> f64 {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |now| now.as_millis())
            .saturating_sub(u128::from(self.unix_ms));
        let projected = if self.unix_ms == 0 {
            0.0
        } else {
            elapsed as f64 * self.speed
        };
        (self.milliseconds as f64 + projected) / 1000.0
    }
}

#[derive(Default)]
pub(super) struct WireState {
    pub(super) logged_in: Option<bool>,
    pub(super) complete: bool,
    pub(super) uri: String,
    pub(super) duration: u64,
    pub(super) position: Position,
    pub(super) position_revision: u64,
    pub(super) status: Status,
    pub(super) volume: Option<u32>,
}

impl WireState {
    pub(super) fn event(&mut self, value: &Value) -> Result<(), String> {
        match value["type"].as_str() {
            Some("auth_state") => self.logged_in = value["logged_in"].as_bool(),
            Some("playback_state") => {
                self.item(&value["item"]);
                self.status = Status::parse(&value["status"]);
                self.update_position(&value["position"]);
                self.volume = volume(&value["volume"]);
                self.complete = true;
            }
            Some("track_changed") => self.item(&value["item"]),
            Some("playback_changed") => self.status = Status::parse(&value["status"]),
            Some("position_sync") => self.update_position(&value["position"]),
            Some("volume_changed") => self.volume = volume(&value["volume"]),
            Some("error") => return Err(COMMAND_ERROR.into()),
            _ => {}
        }
        Ok(())
    }

    fn update_position(&mut self, value: &Value) {
        self.position = Position::parse(value);
        self.position_revision = self.position_revision.wrapping_add(1);
    }

    fn item(&mut self, value: &Value) {
        let uri = value["uri"].as_str().unwrap_or_default();
        if self.uri != uri {
            self.uri = uri.to_owned();
            self.position = Position::default();
        }
        self.duration = value["decorations"]["playback"]["duration_ms"]
            .as_u64()
            .unwrap_or_default();
    }
}

fn volume(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .filter(|value| *value <= 100)
        .map(|value| value as u32)
}

pub(super) struct Client {
    stream: WebSocket<TcpStream>,
    pub(super) state: WireState,
    last_refresh: Instant,
    last_response: Instant,
}

impl Client {
    pub(super) fn connect(endpoint: SocketAddr) -> Result<Self, String> {
        let tcp =
            TcpStream::connect_timeout(&endpoint, CONNECT_TIMEOUT).map_err(|_| DISCONNECTED)?;
        tcp.set_read_timeout(Some(CONNECT_TIMEOUT))
            .map_err(|_| WIRE_ERROR)?;
        tcp.set_write_timeout(Some(CONNECT_TIMEOUT))
            .map_err(|_| WIRE_ERROR)?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(2 * 1024 * 1024))
            .max_frame_size(Some(2 * 1024 * 1024));
        let (stream, _) = client_with_config(format!("ws://{endpoint}/"), tcp, Some(config))
            .map_err(|_| DISCONNECTED)?;
        stream
            .get_ref()
            .set_read_timeout(Some(POLL_INTERVAL))
            .map_err(|_| WIRE_ERROR)?;
        let now = Instant::now();
        Ok(Self {
            stream,
            state: WireState::default(),
            last_refresh: now,
            last_response: now,
        })
    }

    pub(super) fn send(&mut self, command: &str, fields: Value) -> Result<(), String> {
        let mut value = json!({"type": "command", "command": command});
        if let Some(fields) = fields.as_object() {
            for (key, field) in fields {
                value[key] = field.clone();
            }
        }
        self.stream
            .send(Message::Text(value.to_string().into()))
            .map_err(|_| WIRE_ERROR.into())
    }

    pub(super) fn tick(&mut self) -> Result<(), String> {
        if self.state.logged_in == Some(true) && self.last_refresh.elapsed() >= REFRESH_INTERVAL {
            self.send("get_state", json!({}))?;
            self.last_refresh = Instant::now();
        }
        match self.stream.read() {
            Ok(Message::Text(message)) => {
                let value = serde_json::from_str(&message).map_err(|_| WIRE_ERROR)?;
                self.state.event(&value)?;
                self.last_response = Instant::now();
            }
            Ok(Message::Close(_)) => return Err(WIRE_ERROR.into()),
            Ok(_) => {}
            Err(WebSocketError::Io(error))
                if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
            Err(_) => return Err(WIRE_ERROR.into()),
        }
        if self.last_response.elapsed() >= STATE_TIMEOUT {
            return Err(WIRE_ERROR.into());
        }
        Ok(())
    }

    pub(super) fn refresh(&mut self) -> Result<(), String> {
        self.state.complete = false;
        self.send("get_state", json!({}))?;
        self.last_refresh = Instant::now();
        Ok(())
    }

    pub(super) fn wait(
        &mut self,
        duration: Duration,
        cancelled: impl Fn() -> bool,
        ready: impl Fn(&WireState) -> bool,
    ) -> Result<(), String> {
        let deadline = Instant::now() + duration;
        while !cancelled() && Instant::now() < deadline {
            if ready(&self.state) {
                return Ok(());
            }
            self.tick()?;
        }
        Err(COMMAND_ERROR.into())
    }
}
