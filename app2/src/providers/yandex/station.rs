use std::{collections::HashSet, sync::atomic::AtomicBool, time::SystemTime};

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use ureq::Agent;

use crate::{
    playlist::PlaylistTrack,
    providers::{
        ErrorKind, ProviderError,
        transport::{agent, check_cancelled, decode, invalid_response},
    },
};

use super::{
    PROVIDER, api_endpoint, check_access,
    playlists::{track, valid_track_id},
};

const MAX_SEED_LENGTH: usize = 192;
const MAX_SESSION_ID_LENGTH: usize = 256;
const MAX_BATCH_ID_LENGTH: usize = 256;
pub(crate) const MAX_RECENT_TRACKS: usize = 40;
const MAX_BATCH_TRACKS: usize = 3;

#[derive(Debug)]
pub(crate) struct StationBatch {
    pub session_id: String,
    pub batch_id: String,
    pub tracks: Vec<PlaylistTrack>,
}

pub(crate) enum StationFeedback<'a> {
    TrackStarted { track_id: &'a str },
    TrackFinished { track_id: &'a str, seconds: f64 },
    Skip { track_id: &'a str, seconds: f64 },
    Like { track_id: &'a str },
    Dislike { track_id: &'a str, seconds: f64 },
}

pub(crate) fn valid_seed(seed: &str) -> bool {
    if seed.len() > MAX_SEED_LENGTH {
        return false;
    }
    let Some((kind, id)) = seed.split_once(':') else {
        return false;
    };
    match kind {
        "track" => valid_track_id(id),
        "artist" => !id.contains(':') && valid_track_id(id),
        _ => seed_part(kind) && seed_part(id),
    }
}

fn seed_part(part: &str) -> bool {
    !part.is_empty()
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(crate) fn start(
    token: &str,
    seed: &str,
    cancelled: &AtomicBool,
) -> Result<StationBatch, ProviderError> {
    if !valid_seed(seed) {
        return Err(invalid_input(
            "Неизвестный идентификатор станции Яндекс Музыки.",
        ));
    }
    let agent = agent();
    let response = post(
        &agent,
        token,
        "/rotor/session/new",
        json!({"seeds": [seed]}),
        cancelled,
    )?;
    let batch = batch(&response, None, &[])?;
    let event = event("radioStarted", None, None)?;
    post_feedback(
        &agent,
        token,
        &batch.session_id,
        &batch.batch_id,
        event,
        cancelled,
    )?;
    if batch.tracks.is_empty() {
        refill_with(&agent, token, &batch.session_id, &[], cancelled)
    } else {
        Ok(batch)
    }
}

pub(crate) fn refill(
    token: &str,
    session: &str,
    recent: &[String],
    cancelled: &AtomicBool,
) -> Result<StationBatch, ProviderError> {
    refill_with(&agent(), token, session, recent, cancelled)
}

fn refill_with(
    agent: &Agent,
    token: &str,
    session: &str,
    recent: &[String],
    cancelled: &AtomicBool,
) -> Result<StationBatch, ProviderError> {
    let recent = &recent[recent.len().saturating_sub(MAX_RECENT_TRACKS)..];
    if !valid_session_id(session) || recent.iter().any(|id| !valid_track_id(id)) {
        return Err(invalid_input("Некорректная очередь станции Яндекс Музыки."));
    }
    let response = post(
        agent,
        token,
        &format!("/rotor/session/{session}/tracks"),
        json!({"queue": recent}),
        cancelled,
    )?;
    let batch = batch(&response, Some(session), recent)?;
    if batch.tracks.is_empty() {
        return Err(ProviderError::new(
            ErrorKind::Unavailable,
            "Станция Яндекс Музыки не вернула доступных треков.",
        ));
    }
    Ok(batch)
}

pub(crate) fn send_feedback(
    token: &str,
    session: &str,
    batch: &str,
    feedback: StationFeedback<'_>,
    cancelled: &AtomicBool,
) -> Result<(), ProviderError> {
    let (kind, track_id, seconds) = match feedback {
        StationFeedback::TrackStarted { track_id } => ("trackStarted", track_id, None),
        StationFeedback::TrackFinished { track_id, seconds } => {
            ("trackFinished", track_id, Some(seconds))
        }
        StationFeedback::Skip { track_id, seconds } => ("skip", track_id, Some(seconds)),
        StationFeedback::Like { track_id } => ("like", track_id, None),
        StationFeedback::Dislike { track_id, seconds } => ("dislike", track_id, Some(seconds)),
    };
    let event = event(kind, Some(track_id), seconds)?;
    post_feedback(&agent(), token, session, batch, event, cancelled)
}

fn post_feedback(
    agent: &Agent,
    token: &str,
    session: &str,
    batch: &str,
    event: Value,
    cancelled: &AtomicBool,
) -> Result<(), ProviderError> {
    if !valid_session_id(session) || !valid_batch_id(batch) {
        return Err(invalid_input("Некорректная сессия станции Яндекс Музыки."));
    }
    let mut body = json!({"event": event});
    if !batch.is_empty() {
        body["batchId"] = json!(batch);
    }
    let response = post(
        agent,
        token,
        &format!("/rotor/session/{session}/feedback"),
        body,
        cancelled,
    )?;
    if !result(&response).is_object() {
        return Err(invalid_response(PROVIDER));
    }
    Ok(())
}

fn event(kind: &str, track_id: Option<&str>, seconds: Option<f64>) -> Result<Value, ProviderError> {
    if track_id.is_some_and(|id| !valid_track_id(id))
        || seconds.is_some_and(|value| !value.is_finite() || value < 0.)
    {
        return Err(invalid_input("Некорректное событие станции Яндекс Музыки."));
    }
    let timestamp =
        DateTime::<Utc>::from(SystemTime::now()).to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut event = json!({"type": kind, "timestamp": timestamp});
    if let Some(track_id) = track_id {
        event["trackId"] = json!(track_id);
    }
    if let Some(seconds) = seconds {
        event["totalPlayedSeconds"] = json!(seconds);
    }
    Ok(event)
}

fn post(
    agent: &Agent,
    token: &str,
    path: &str,
    body: Value,
    cancelled: &AtomicBool,
) -> Result<Value, ProviderError> {
    check_cancelled(cancelled)?;
    check_access(token)?;
    let response = decode(
        PROVIDER,
        agent
            .post(&api_endpoint(path))
            .header("Authorization", format!("OAuth {token}"))
            .send_json(body),
    );
    check_cancelled(cancelled)?;
    response
}

fn result(response: &Value) -> &Value {
    response.get("result").unwrap_or(response)
}

fn batch(
    response: &Value,
    expected_session: Option<&str>,
    recent: &[String],
) -> Result<StationBatch, ProviderError> {
    let data = result(response);
    if !data.is_object() {
        return Err(invalid_response(PROVIDER));
    }
    if data["terminated"] == true || data["unknownSession"] == true {
        return Err(ProviderError::new(
            ErrorKind::Unavailable,
            "Сессия станции Яндекс Музыки завершилась. Запустите её снова.",
        ));
    }
    let session_id = match data.get("radioSessionId") {
        None | Some(Value::Null) => expected_session,
        Some(value) => value
            .as_str()
            .filter(|session| expected_session.is_none_or(|expected| *session == expected)),
    }
    .filter(|session| valid_session_id(session))
    .ok_or_else(|| invalid_response(PROVIDER))?
    .to_owned();
    let batch_id = match data.get("batchId") {
        None | Some(Value::Null) => Some(""),
        Some(value) => value.as_str(),
    }
    .filter(|id| valid_batch_id(id))
    .ok_or_else(|| invalid_response(PROVIDER))?
    .to_owned();
    let rows = match data.get("sequence") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(rows)) => rows.as_slice(),
        Some(_) => return Err(invalid_response(PROVIDER)),
    };
    let mut seen: HashSet<_> = recent.iter().map(|id| recording(id).to_owned()).collect();
    let tracks = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            row["track"].is_object() && row["type"].as_str().is_none_or(|kind| kind == "track")
        })
        .map(|(position, row)| track(row, position))
        .filter(|track| track.media.is_some() && seen.insert(recording(&track.id).to_owned()))
        .take(MAX_BATCH_TRACKS)
        .collect();
    Ok(StationBatch {
        session_id,
        batch_id,
        tracks,
    })
}

fn recording(id: &str) -> &str {
    id.split_once(':').map_or(id, |(track, _)| track)
}

fn valid_session_id(id: &str) -> bool {
    id.len() <= MAX_SESSION_ID_LENGTH
        && id.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'~'))
        })
}

fn valid_batch_id(id: &str) -> bool {
    id.len() <= MAX_BATCH_ID_LENGTH && id.bytes().all(|byte| byte.is_ascii_graphic())
}

fn invalid_input(message: &str) -> ProviderError {
    ProviderError::new(ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playlist::Media;

    #[test]
    fn selects_playable_recommendations_with_canonical_track_ids() {
        let response = json!({
            "result": {
                "radioSessionId": "session~123.uk",
                "batchId": "batch/1+2=",
                "sequence": [
                    {"type": "ad"},
                    {"type": "track", "track": {"id": 1, "title": "Unavailable", "available": false}},
                    {"type": "track", "track": {"id": 2, "title": "Already played", "albums": [{"id": 22}]}},
                    {"type": "track", "track": {"id": 3, "title": "New", "artists": [{"name": "Artist"}], "albums": [{"id": 33, "title": "Album"}], "durationMs": 103000}},
                    {"type": "track", "track": {"id": 3, "title": "Duplicate", "albums": [{"id": 34}]}},
                    {"type": "track", "track": {"id": 4, "title": "Another"}},
                    {"type": "track", "track": {"id": 5, "title": "Third"}},
                    {"type": "track", "track": {"id": 6, "title": "Excess"}}
                ]
            }
        });
        let actual = batch(&response, None, &["2:21".into()]).expect("valid recommendation batch");
        assert_eq!(actual.session_id, "session~123.uk");
        assert_eq!(actual.batch_id, "batch/1+2=");
        assert_eq!(
            actual
                .tracks
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["3:33", "4", "5"]
        );
        let track = &actual.tracks[0];
        assert_eq!(track.duration, Some(103));
        assert_eq!(track.artist, "Artist");
        assert_eq!(track.album.as_deref(), Some("Album"));
        assert_eq!(
            track.media,
            Some(Media::Provider {
                provider: PROVIDER,
                id: "3:33".into()
            })
        );
    }

    #[test]
    fn refill_inherits_session_and_refuses_a_replaced_session() {
        let response = json!({"result": {"batchId": "next", "sequence": []}});
        let actual = batch(&response, Some("original"), &[]).expect("original session");
        assert_eq!(actual.session_id, "original");
        assert!(actual.tracks.is_empty());

        let changed = json!({"result": {"radioSessionId": "different", "sequence": []}});
        assert_eq!(
            batch(&changed, Some("original"), &[])
                .expect_err("session mismatch")
                .kind,
            ErrorKind::InvalidResponse
        );
    }

    #[test]
    fn finished_and_missing_sessions_are_unavailable() {
        for response in [
            json!({"result": {"terminated": true}}),
            json!({"result": {"unknownSession": true}}),
        ] {
            assert_eq!(
                batch(&response, Some("original"), &[])
                    .expect_err("unavailable session")
                    .kind,
                ErrorKind::Unavailable
            );
        }
    }

    #[test]
    fn validates_seed_session_and_feedback_without_network() {
        for seed in [
            "user:onyourwave",
            "genre:pop-rock",
            "mood:calm",
            "track:12:13",
            "artist:21",
        ] {
            assert!(valid_seed(seed), "{seed}");
        }
        for seed in ["", "track:1/x", "artist:2:3", "genre:../pop", "station"] {
            assert!(!valid_seed(seed), "{seed}");
        }
        assert!(valid_session_id("seed~123.account"));
        assert!(!valid_session_id("seed/../account"));
        assert!(!valid_session_id("seed..account"));

        let event_data = event("skip", Some("21:12"), Some(1.5)).expect("valid skip");
        assert_eq!(event_data["type"], "skip");
        assert_eq!(event_data["trackId"], "21:12");
        assert_eq!(event_data["totalPlayedSeconds"], 1.5);
        let timestamp = event_data["timestamp"].as_str().expect("timestamp");
        assert!(DateTime::parse_from_rfc3339(timestamp).is_ok());
        assert!(event("skip", Some("21:12"), Some(f64::NAN)).is_err());
        assert_eq!(
            start("", "user:onyourwave", &AtomicBool::new(true))
                .expect_err("cancelled start")
                .kind,
            ErrorKind::Cancelled
        );
    }
}
