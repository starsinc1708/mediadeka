mod html;
mod matching;

use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use ureq::{Body, http::Response};
use url::Url;

use crate::{
    music::{
        lyrics::{Recording, response_max_age, search_spelling, search_terms},
        normalized,
    },
    telemetry,
};

use super::transport::agent;
use matching::{SearchMatch, unique_hit};

const API: &str = "https://api.genius.com";
const MAX_BODY_BYTES: u64 = 3 * 1024 * 1024;
const REFERENTS_PER_PAGE: usize = 20;
const MAX_REFERENT_PAGES: u32 = 8;
const CANCELLED: &str = "Запрос Genius отменён.";

#[cfg(all(test, windows, feature = "capture"))]
pub(crate) mod test_server {
    use std::{net::SocketAddr, sync::OnceLock};

    static ENDPOINT: OnceLock<SocketAddr> = OnceLock::new();

    pub(crate) fn use_loopback(address: SocketAddr) {
        ENDPOINT.set(address).expect("isolated Genius test process");
    }

    pub(super) fn endpoint() -> Option<url::Url> {
        ENDPOINT
            .get()
            .map(|host| url::Url::parse(&format!("http://{host}")).unwrap())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Details {
    pub id: u64,
    pub title: String,
    pub url: String,
    pub artist: String,
    pub writers: Vec<String>,
    pub producers: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Annotation {
    pub id: u64,
    pub fragment: String,
    pub text: String,
}

pub(crate) struct ResponseData<T> {
    pub value: T,
    pub max_age: Option<i64>,
}

#[derive(Deserialize)]
struct Envelope<T> {
    meta: Meta,
    response: Option<T>,
}

#[derive(Deserialize)]
struct Meta {
    status: u16,
}

#[derive(Deserialize)]
struct Hits {
    hits: Vec<Hit>,
}

#[derive(Deserialize)]
struct Hit {
    #[serde(rename = "type")]
    kind: String,
    result: serde_json::Value,
}

#[derive(Deserialize)]
struct SongResponse {
    song: Song,
}

#[derive(Deserialize)]
struct Song {
    id: u64,
    title: String,
    artist_names: Option<String>,
    primary_artist: Option<Person>,
    #[serde(default)]
    primary_artists: Vec<Person>,
    #[serde(default)]
    featured_artists: Vec<Person>,
    album: Option<Person>,
    duration_ms: Option<u64>,
    duration: Option<f64>,
    url: Option<String>,
    #[serde(default)]
    writer_artists: Vec<Person>,
    #[serde(default)]
    producer_artists: Vec<Person>,
}

#[derive(Deserialize)]
struct Person {
    name: String,
}

#[derive(Deserialize)]
struct Referents {
    referents: Vec<Referent>,
    next_page: Option<u32>,
}

#[derive(Deserialize)]
struct Referent {
    fragment: String,
    #[serde(default)]
    annotations: Vec<AnnotationWire>,
}

#[derive(Deserialize)]
struct AnnotationWire {
    id: u64,
    body: AnnotationBody,
}

#[derive(Deserialize)]
struct AnnotationBody {
    plain: Option<String>,
}

pub(crate) fn validate_token(token: &str, cancelled: &AtomicBool) -> Result<(), String> {
    if token.is_empty() || token.len() > 2_000 || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("Введите Client Access Token Genius без пробелов.".into());
    }
    api::<Hits>(token, "/search", &[("q", "Genius")], cancelled)?;
    Ok(())
}

pub(crate) fn details(
    token: &str,
    track: &Recording,
    cancelled: &AtomicBool,
) -> Result<Option<ResponseData<Details>>, String> {
    if normalized(&track.title).is_empty() || normalized(&track.artist).is_empty() {
        return Ok(None);
    }
    let query = format!(
        "{} {}",
        search_spelling(&track.title),
        search_spelling(&track.artist)
    );
    let hits: ResponseData<Hits> = api(token, "/search", &[("q", &query)], cancelled)?;
    let (id, search_age) = match unique_hit(track, hits.value.hits) {
        SearchMatch::Unique(id) => (id, hits.max_age),
        SearchMatch::Ambiguous => return Ok(None),
        SearchMatch::Missing => {
            let Some((title, artist)) = search_terms(track) else {
                return Ok(None);
            };
            let query = format!("{title} {artist}");
            let hits: ResponseData<Hits> = api(token, "/search", &[("q", &query)], cancelled)?;
            let SearchMatch::Unique(id) = unique_hit(track, hits.value.hits) else {
                return Ok(None);
            };
            (id, hits.max_age)
        }
    };
    let result: ResponseData<SongResponse> = api(
        token,
        &format!("/songs/{id}"),
        &[("text_format", "plain")],
        cancelled,
    )?;
    let song = result.value.song;
    if song.id != id || !song.matches(track) {
        return Ok(None);
    }
    let Some(url) = song.url.as_deref().and_then(song_url) else {
        return Err("Genius не вернул корректную ссылку на песню.".into());
    };
    Ok(Some(ResponseData {
        max_age: shorter(search_age, result.max_age),
        value: Details {
            id,
            artist: song
                .primary_artist
                .map(|artist| artist.name)
                .or(song.artist_names)
                .unwrap_or_default(),
            title: song.title,
            url: url.into(),
            writers: song.writer_artists.into_iter().map(|p| p.name).collect(),
            producers: song.producer_artists.into_iter().map(|p| p.name).collect(),
        },
    }))
}

pub(crate) fn annotations(
    token: &str,
    id: u64,
    cancelled: &AtomicBool,
) -> Result<ResponseData<Vec<Annotation>>, String> {
    let song_id = id.to_string();
    let per_page = REFERENTS_PER_PAGE.to_string();
    let mut seen = BTreeSet::new();
    let mut annotations = Vec::new();
    let mut page = 1u32;
    let mut max_age = None;
    for index in 0..MAX_REFERENT_PAGES {
        let result: ResponseData<Referents> = api(
            token,
            "/referents",
            &[
                ("song_id", &song_id),
                ("text_format", "plain"),
                ("per_page", &per_page),
                ("page", &page.to_string()),
            ],
            cancelled,
        )?;
        max_age = if index == 0 {
            result.max_age
        } else {
            shorter(max_age, result.max_age)
        };
        let referents = result.value.referents;
        let full_page = referents.len() == REFERENTS_PER_PAGE;
        if referents.is_empty() {
            break;
        }
        for referent in referents {
            let fragment = referent.fragment.trim();
            if fragment.is_empty() {
                continue;
            }
            for annotation in referent.annotations {
                if let Some(text) = annotation.body.plain
                    && !text.trim().is_empty()
                    && seen.insert(annotation.id)
                {
                    annotations.push(Annotation {
                        id: annotation.id,
                        fragment: fragment.into(),
                        text,
                    });
                }
            }
        }
        let next = result
            .value
            .next_page
            .or(full_page.then_some(page.saturating_add(1)));
        let Some(next) = next.filter(|&next| next > page) else {
            break;
        };
        page = next;
    }
    Ok(ResponseData {
        value: annotations,
        max_age,
    })
}

pub(crate) fn lyric_page(
    url: &str,
    cancelled: &AtomicBool,
) -> Result<ResponseData<Option<String>>, String> {
    let url = song_url(url).ok_or("Некорректная ссылка на Genius.")?;
    let mut response = get(&url, None, cancelled)?;
    let max_age = response_max_age(response.headers());
    let text = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_string()
        .map_err(|_| "Не удалось прочитать страницу Genius.")?;
    check_cancelled(cancelled)?;
    Ok(ResponseData {
        value: html::lyrics(&text),
        max_age,
    })
}

pub(crate) fn song_url(value: &str) -> Option<Url> {
    let mut url = Url::parse(value).ok()?;
    if !matches!(url.host_str(), Some("genius.com" | "www.genius.com"))
        || url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path().trim_matches('/').is_empty()
    {
        return None;
    }
    url.set_host(Some("genius.com")).ok()?;
    url.set_query(None);
    url.set_fragment(None);
    Some(url)
}

fn shorter(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    a.zip(b).map(|(a, b)| a.min(b))
}

fn api<T: DeserializeOwned>(
    token: &str,
    path: &str,
    parameters: &[(&str, &str)],
    cancelled: &AtomicBool,
) -> Result<ResponseData<T>, String> {
    let base = Url::parse(API).expect("fixed Genius API URL");
    #[cfg(all(test, windows, feature = "capture"))]
    let base = test_server::endpoint().unwrap_or(base);
    let mut url = base;
    url.set_path(path);
    url.query_pairs_mut().extend_pairs(parameters);
    let mut response = get(&url, Some(token), cancelled)?;
    let max_age = response_max_age(response.headers());
    let reader = telemetry::api_body(
        response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .reader(),
    );
    let payload: Envelope<T> = serde_json::from_reader(std::io::BufReader::new(reader))
        .map_err(|_| "Genius вернул неожиданные данные.")?;
    check_cancelled(cancelled)?;
    let status = payload.meta.status;
    if status != 200 {
        return Err(format!("Genius вернул код {status}."));
    }
    let value = payload.response.ok_or("Genius вернул пустой ответ.")?;
    Ok(ResponseData { value, max_age })
}

fn get(url: &Url, token: Option<&str>, cancelled: &AtomicBool) -> Result<Response<Body>, String> {
    check_cancelled(cancelled)?;
    let client = agent();
    let mut request = client.get(url.as_str());
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = request.call().map_err(|_| "Нет ответа от Genius.")?;
    check_cancelled(cancelled)?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("Genius вернул HTTP {}.", status.as_u16()));
    }
    Ok(response)
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Acquire) {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
