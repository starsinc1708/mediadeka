

use serde::{Deserialize, Serialize};
use ureq::http::{StatusCode, header::AUTHORIZATION};

use super::{Play, read_json};

const API: &str = "https://api.listenbrainz.org";
const CLIENT: &str = "Mediadeka";
const NO_RESPONSE: &str = "Нет ответа от ListenBrainz. Проверьте соединение.";
const TOKEN_REFUSED: &str = "ListenBrainz отклонил токен.";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::integrations) struct ListenBrainzAccount {
    token: String,
    pub(in crate::integrations) name: String,
    #[cfg(test)]
    #[serde(skip)]
    endpoint: Option<String>,
}

pub(in crate::integrations) fn listenbrainz_connect(
    token: String,
) -> Result<ListenBrainzAccount, String> {
    connect_at(API, token)
}

pub(super) fn connect_at(api: &str, token: String) -> Result<ListenBrainzAccount, String> {
    let token = token.trim().to_owned();
    if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err("Введите токен ListenBrainz.".into());
    }

    let mut response = crate::providers::http_agent()
        .get(format!("{api}/1/validate-token"))
        .header(AUTHORIZATION, format!("Token {token}"))
        .call()
        .map_err(|_| NO_RESPONSE)?;
    check_status(response.status())?;

    let answer: Validation =
        read_json(&mut response).ok_or("ListenBrainz вернул неожиданный ответ.")?;
    if !answer.valid {
        return Err(TOKEN_REFUSED.into());
    }
    Ok(ListenBrainzAccount {
        token,
        name: answer.user_name.unwrap_or_default(),
        #[cfg(test)]
        endpoint: Some(api.into()),
    })
}

impl ListenBrainzAccount {
    #[cfg(test)]
    pub(in crate::integrations) fn test_at(endpoint: String) -> Self {
        Self {
            token: "fixture-token".into(),
            name: "ListenBrainz fixture".into(),
            endpoint: Some(endpoint),
        }
    }

    pub(in crate::integrations) fn now_playing(&self, play: &Play) -> Result<(), String> {
        self.submit("playing_now", &[listen(play, false)])
    }

    pub(in crate::integrations) fn scrobble(&self, play: &Play) -> Result<(), String> {
        self.submit("single", &[listen(play, true)])
    }

    fn submit(&self, kind: &str, payload: &[Listen<'_>]) -> Result<(), String> {
        #[cfg(test)]
        let api = self.endpoint.as_deref().unwrap_or(API);
        #[cfg(not(test))]
        let api = API;
        let answer = crate::providers::http_agent()
            .post(format!("{api}/1/submit-listens"))
            .header(AUTHORIZATION, format!("Token {}", self.token))
            .send_json(&Submission {
                listen_type: kind,
                payload,
            })
            .map_err(|_| NO_RESPONSE)?;
        check_status(answer.status())
    }
}

fn check_status(status: StatusCode) -> Result<(), String> {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(TOKEN_REFUSED.into()),
        status if status.is_success() => Ok(()),
        _ => Err(format!("ListenBrainz вернул HTTP {}.", status.as_u16())),
    }
}

/// Builds one listen. The now-playing report carries no start time.
fn listen(play: &Play, timed: bool) -> Listen<'_> {
    Listen {
        listened_at: timed.then(|| play.timestamp()),
        track_metadata: Metadata {
            artist_name: &play.artist,
            track_name: &play.title,
            release_name: play.release(),
            additional_info: Extra {
                duration_ms: u64::try_from(play.duration.as_millis()).unwrap_or(u64::MAX),
                media_player: CLIENT,
                submission_client: CLIENT,
            },
        },
    }
}

#[derive(Deserialize)]
struct Validation {
    valid: bool,
    user_name: Option<String>,
}

#[derive(Serialize)]
struct Submission<'a> {
    listen_type: &'a str,
    payload: &'a [Listen<'a>],
}

#[derive(Serialize)]
struct Listen<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    listened_at: Option<i64>,
    track_metadata: Metadata<'a>,
}

#[derive(Serialize)]
struct Metadata<'a> {
    artist_name: &'a str,
    track_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_name: Option<&'a str>,
    additional_info: Extra,
}

#[derive(Serialize)]
struct Extra {
    duration_ms: u64,
    media_player: &'static str,
    submission_client: &'static str,
}
