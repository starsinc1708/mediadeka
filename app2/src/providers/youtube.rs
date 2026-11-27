use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use crate::{
    music::{ObjectKind, ObjectRef},
    playlist::ArtistCredit,
};

use super::{Credentials, ErrorKind, ProviderError, ProviderId, transport::*};

mod account;
pub(super) mod catalog;
mod collection;
mod feed;
pub(super) mod likes;
pub(super) mod lyrics;
mod playlist;

pub use account::{YouTubeChoices, youtube_profiles};
pub(super) use account::{restore, sign_in};
pub(super) use collection::read_collection;
pub use feed::home;
pub(super) use feed::is_mix_shelf;
pub(super) use playlist::{playlist_with_progress, valid_playlist_id};

const PROVIDER: ProviderId = ProviderId::YouTubeMusic;
const MUSIC_ORIGIN: &str = "https://music.youtube.com";
const WEB_ORIGIN: &str = "https://www.youtube.com";
const BROWSER: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";
const SIGNED_IN_COOKIES: &[&str] = &["SAPISID", "__Secure-3PAPISID"];

fn proof(cookies: &str) -> Option<&str> {
    for name in SIGNED_IN_COOKIES {
        if let Some(value) = cookies
            .split(';')
            .filter_map(|pair| pair.trim().split_once('='))
            .find_map(|(key, value)| (key == *name && !value.is_empty()).then_some(value))
        {
            return Some(value);
        }
    }
    None
}

#[derive(Clone, Copy)]
struct Auth<'a> {
    cookies: &'a str,
    index: usize,
    page_id: Option<&'a str>,
}

impl<'a> Auth<'a> {
    fn from_credentials(credentials: &'a Credentials) -> Self {
        let Credentials::YouTubeMusic {
            cookies,
            index,
            page_id,
        } = credentials
        else {
            unreachable!()
        };
        Self {
            cookies,
            index: *index,
            page_id: page_id.as_deref(),
        }
    }
}

fn request_with_agent(
    agent: &ureq::Agent,
    web: bool,
    endpoint: &str,
    mut body: Value,
    auth: Option<Auth<'_>>,
    visitor: Option<&str>,
    timeout: Option<Duration>,
) -> Result<Value, ProviderError> {
    let (origin, name, version, number) = if web {
        (WEB_ORIGIN, "WEB", "2.20260707.00.00", "1")
    } else {
        (MUSIC_ORIGIN, "WEB_REMIX", "1.20260707.12.00", "67")
    };
    let region = body["context"]["client"]["gl"].take();
    let region = region
        .as_str()
        .filter(|value| value.len() == 2 && value.bytes().all(|byte| byte.is_ascii_uppercase()))
        .unwrap_or("US");
    body["context"] = json!({"client": {
        "clientName": name, "clientVersion": version, "hl": "ru", "gl": region, "platform": "DESKTOP",
        "originalUrl": origin, "userAgent": BROWSER,
    }});
    if !web {
        body["isAudioOnly"] = true.into();
    }
    let mut request = agent
        .post(&format!(
            "{origin}/youtubei/v1/{endpoint}?prettyPrint=false&alt=json"
        ))
        .header("Origin", origin)
        .header("User-Agent", BROWSER)
        .header("Accept", "application/json")
        .header("X-Youtube-Client-Name", number)
        .header("X-Youtube-Client-Version", version);
    if let Some(timeout) = timeout {
        request = request.config().timeout_global(Some(timeout)).build();
    }
    if let Some(visitor) = visitor.filter(|visitor| valid_browse_token(visitor)) {
        body["context"]["client"]["visitorData"] = visitor.into();
        request = request.header("X-Goog-Visitor-Id", visitor);
    }
    if let Some(auth) = auth {
        let sapisid = proof(auth.cookies).ok_or_else(|| {
            ProviderError::new(
                ErrorKind::Expired,
                "Подключение YouTube Music недействительно.",
            )
        })?;
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let digest = Sha1::digest(format!("{epoch} {sapisid} {origin}"));
        request = request
            .header("Cookie", auth.cookies)
            .header("X-Origin", origin)
            .header("X-Goog-AuthUser", auth.index.to_string())
            .header("Authorization", format!("SAPISIDHASH {epoch}_{digest:x}"));
        if let Some(page) = auth.page_id {
            request = request.header("X-Goog-PageId", page);
        }
    }
    decode(PROVIDER, request.send_json(body)).map_err(|error| {
        if auth.is_none() && error.kind == ErrorKind::Rejected {
            ProviderError::new(ErrorKind::Network, error.message)
        } else {
            error
        }
    })
}

fn valid_browse_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 4096
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.~+/=%".contains(&byte))
}

fn signed_out(response: &Value) -> bool {
    let context = &response["responseContext"];
    let web = &context["mainAppWebResponseContext"]["loggedOut"];
    web.as_bool().unwrap_or_else(|| {
        context["serviceTrackingParams"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|entry| entry["service"] == "GFEEDBACK")
            .flat_map(|entry| entry["params"].as_array().into_iter().flatten())
            .any(|field| field["key"] == "logged_in" && field["value"] == "0")
    })
}

fn text(value: &Value) -> Option<String> {
    let string = value
        .as_str()
        .or_else(|| value["simpleText"].as_str())
        .map(str::to_owned)
        .or_else(|| {
            value["runs"].as_array().map(|runs| {
                runs.iter()
                    .filter_map(|run| run["text"].as_str())
                    .collect::<String>()
            })
        })?;
    let string = string.trim();
    (!string.is_empty()).then(|| string.to_owned())
}

fn columns(row: &Value) -> impl Iterator<Item = &Value> + Clone {
    row["flexColumns"]
        .as_array()
        .into_iter()
        .flatten()
        .skip(1)
        .map(|column| &column["musicResponsiveListItemFlexColumnRenderer"]["text"])
}

fn runs(value: &Value) -> impl Iterator<Item = &Value> {
    value["runs"].as_array().into_iter().flatten()
}

fn browse_id(run: &Value) -> Option<&str> {
    run["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str()
}

fn artist_endpoint(endpoint: &Value) -> Option<ObjectRef> {
    if endpoint["browseEndpointContextSupportedConfigs"]["browseEndpointContextMusicConfig"]["pageType"]
        != "MUSIC_PAGE_TYPE_ARTIST"
    {
        return None;
    }
    let id = field(endpoint, "browseId")?;
    id.starts_with("UC")
        .then(|| ObjectRef::native(PROVIDER, ObjectKind::Artist, id))
        .flatten()
}

fn album_run(value: &Value) -> Option<&Value> {
    runs(value).find(|run| album_browse_id(run).is_some())
}

fn album_ref(run: &Value) -> Option<ObjectRef> {
    ObjectRef::native(
        PROVIDER,
        ObjectKind::Album,
        album_browse_id(run)?.to_owned(),
    )
}

fn album_browse_id(run: &Value) -> Option<&str> {
    browse_id(run).filter(|id| id.starts_with("MPRE") && feed::safe_id(id))
}

fn artist_names(credits: &[ArtistCredit]) -> Vec<String> {
    credits
        .iter()
        .filter(|credit| credit.reference.is_some())
        .map(|credit| credit.name.clone())
        .collect()
}

fn complete_artist_names(credits: &[ArtistCredit]) -> Option<Vec<String>> {
    credits
        .iter()
        .map(|credit| credit.reference.as_ref().map(|_| credit.name.clone()))
        .collect()
}

fn artist_browse_id(run: &Value) -> Option<&str> {
    browse_id(run).filter(|id| id.starts_with("UC") && feed::safe_id(id))
}

fn artist_credits(value: &Value) -> impl Iterator<Item = ArtistCredit> + '_ {
    value["runs"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .split(|run| matches!(run["text"].as_str().map(str::trim), Some("•" | "·")))
        .filter(|group| group.iter().any(|run| artist_browse_id(run).is_some()))
        .flatten()
        .filter_map(|run| {
            let name = field(run, "text")?;
            let id = artist_browse_id(run).map(str::to_owned);
            if id.is_none()
                && matches!(
                    name.as_str(),
                    "," | "&" | "и" | "and" | "x" | "×" | "feat." | "ft."
                )
            {
                return None;
            }
            Some(ArtistCredit::from_catalog(name, PROVIDER, id))
        })
}

fn named<'a>(value: &'a Value, key: &str, found: &mut Vec<&'a Value>) {
    match value {
        Value::Object(map) => {
            if let Some(node) = map.get(key) {
                found.push(node);
            }
            for node in map.values() {
                named(node, key, found);
            }
        }
        Value::Array(values) => {
            for node in values {
                named(node, key, found);
            }
        }
        _ => {}
    }
}
