mod callback;

use std::sync::atomic::AtomicBool;

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use url::Url;

use super::{Play, read_json};

const API: &str = "https://ws.audioscrobbler.com/2.0/";
const AUTHORIZE: &str = "https://www.last.fm/api/auth/";
const CALLBACK_PORT: u16 = 8990;
const KEY_REFUSED: [u32; 2] = [10, 26];
const INVALID_RESPONSE: &str = "Last.fm вернул неожиданный ответ.";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::integrations) struct LastFmAccount {
    key: String,
    secret: String,
    session: String,
    pub(in crate::integrations) name: String,
    #[cfg(test)]
    #[serde(skip)]
    endpoint: Option<String>,
}

pub(in crate::integrations) fn last_fm_connect(
    key: String,
    secret: String,
    cancelled: &AtomicBool,
    open: impl FnOnce(&str) -> Result<(), String>,
) -> Result<LastFmAccount, String> {
    connect_at(API, AUTHORIZE, CALLBACK_PORT, key, secret, cancelled, open)
}

pub(super) fn connect_at(
    endpoint: &str,
    authorize: &str,
    port: u16,
    key: String,
    secret: String,
    cancelled: &AtomicBool,
    open: impl FnOnce(&str) -> Result<(), String>,
) -> Result<LastFmAccount, String> {
    let key = key.trim().to_owned();
    let secret = secret.trim().to_owned();
    if key.is_empty() || secret.is_empty() {
        return Err("Введите ключ API и секрет Last.fm.".into());
    }
    callback::check_cancelled(cancelled)?;

    // Check the API key before opening approval, since a bad key never comes back.
    post(
        endpoint,
        &secret,
        vec![("method", "auth.getToken".into()), ("api_key", key.clone())],
    )?;
    callback::check_cancelled(cancelled)?;

    let browser = callback::Approval::bind(port)?;
    let mut authorize = Url::parse(authorize).map_err(|_| INVALID_RESPONSE)?;
    authorize
        .query_pairs_mut()
        .extend_pairs([("api_key", key.as_str()), ("cb", browser.url().as_str())]);
    open(authorize.as_str())?;
    let token = browser.token(cancelled)?;
    callback::check_cancelled(cancelled)?;

    let answer = post(
        endpoint,
        &secret,
        vec![
            ("method", "auth.getSession".into()),
            ("api_key", key.clone()),
            ("token", token),
        ],
    )?;
    callback::check_cancelled(cancelled)?;
    let granted = answer
        .session
        .filter(|session| !session.key.is_empty())
        .ok_or(INVALID_RESPONSE)?;
    Ok(LastFmAccount {
        key,
        secret,
        session: granted.key,
        name: granted.name,
        #[cfg(test)]
        endpoint: Some(endpoint.into()),
    })
}

impl LastFmAccount {
    #[cfg(test)]
    pub(in crate::integrations) fn test_at(endpoint: String) -> Self {
        Self {
            key: "fixture-key".into(),
            secret: "fixture-secret".into(),
            session: "fixture-session".into(),
            name: "Last.fm fixture".into(),
            endpoint: Some(endpoint),
        }
    }

    pub(in crate::integrations) fn now_playing(&self, play: &Play) -> Result<(), String> {
        let mut form = vec![
            ("method", "track.updateNowPlaying".into()),
            ("artist", play.artist.clone()),
            ("track", play.title.clone()),
            ("duration", play.duration.as_secs().to_string()),
        ];
        if let Some(album) = play.release() {
            form.push(("album", album.into()));
        }
        let result = self.call(form)?.nowplaying.ok_or(INVALID_RESPONSE)?;
        if result.ignored_message.code.number()? != 0 {
            return Err("Last.fm не принял данные о текущей записи.".into());
        }
        Ok(())
    }

    pub(in crate::integrations) fn scrobble(&self, play: &Play) -> Result<(), String> {
        let mut form = vec![
            ("method", "track.scrobble".into()),
            ("artist[0]", play.artist.clone()),
            ("track[0]", play.title.clone()),
            ("timestamp[0]", play.timestamp().to_string()),
            ("duration[0]", play.duration.as_secs().to_string()),
        ];
        if let Some(album) = play.release() {
            form.push(("album[0]", album.into()));
        }
        let result = self.call(form)?.scrobbles.ok_or(INVALID_RESPONSE)?;
        if result.counts.accepted.number()? == 0 {
            return Err("Last.fm не засчитал прослушивание.".into());
        }
        Ok(())
    }

    fn call(&self, mut form: Vec<(&str, String)>) -> Result<Answer, String> {
        form.push(("api_key", self.key.clone()));
        form.push(("sk", self.session.clone()));
        #[cfg(test)]
        let endpoint = self.endpoint.as_deref().unwrap_or(API);
        #[cfg(not(test))]
        let endpoint = API;
        post(endpoint, &self.secret, form)
    }
}

#[derive(Deserialize)]
struct Answer {
    error: Option<u32>,
    session: Option<Granted>,
    nowplaying: Option<NowPlaying>,
    scrobbles: Option<Scrobbles>,
}

#[derive(Deserialize)]
struct Granted {
    key: String,
    name: String,
}

#[derive(Deserialize)]
struct NowPlaying {
    #[serde(rename = "ignoredMessage")]
    ignored_message: IgnoredMessage,
}

#[derive(Deserialize)]
struct IgnoredMessage {
    code: Count,
}

#[derive(Deserialize)]
struct Scrobbles {
    #[serde(rename = "@attr")]
    counts: ScrobbleCounts,
}

#[derive(Deserialize)]
struct ScrobbleCounts {
    accepted: Count,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Count {
    Number(u32),
    Text(String),
}

impl Count {
    fn number(self) -> Result<u32, String> {
        match self {
            Self::Number(number) => Ok(number),
            Self::Text(text) => text.parse().map_err(|_| INVALID_RESPONSE.into()),
        }
    }
}

/// Signs the sorted form before adding the response format, then posts it to Last.fm.
fn post(endpoint: &str, secret: &str, mut form: Vec<(&str, String)>) -> Result<Answer, String> {
    form.sort_by(|left, right| left.0.cmp(right.0));
    form.push(("api_sig", signature(&form, secret)));
    form.push(("format", "json".into()));

    let mut response = crate::providers::http_agent()
        .post(endpoint)
        .send_form(form)
        .map_err(|_| "Нет ответа от Last.fm. Проверьте соединение.")?;
    let answer: Option<Answer> = read_json(&mut response);
    if let Some(code) = answer.as_ref().and_then(|answer| answer.error) {
        return Err(match code {
            code if KEY_REFUSED.contains(&code) => "Last.fm отклонил ключ API.".into(),
            9 => "Сессия Last.fm недействительна. Подключите учётную запись заново.".into(),
            _ => format!("Last.fm отклонил запрос (код {code})."),
        });
    }
    if !response.status().is_success() {
        return Err(format!(
            "Last.fm вернул HTTP {}.",
            response.status().as_u16()
        ));
    }
    answer.ok_or_else(|| INVALID_RESPONSE.into())
}

fn signature(form: &[(&str, String)], secret: &str) -> String {
    let mut base = String::new();
    for (name, value) in form {
        base.push_str(name);
        base.push_str(value);
    }
    base.push_str(secret);
    format!("{:x}", Md5::digest(base.as_bytes()))
}
