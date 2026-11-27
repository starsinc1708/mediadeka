use std::{
    io::{BufReader, Read},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime},
};

use serde_json::Value;
use ureq::{Agent, Body, http::Response};
use url::Url;

use super::{PublicLink, public_link};
use crate::telemetry;

const BODY_LIMIT: u64 = 8 * 1024 * 1024;
const RETRY_DELAY: Duration = Duration::from_secs(60);
const MAX_PUBLIC_REDIRECTS: usize = 2;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum LookupError {
    Missing,
    Unsupported,
    Temporary(Duration),
    Cancelled,
}

pub(super) type Result<T> = std::result::Result<T, LookupError>;

pub(super) fn client() -> Agent {
    Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(Duration::from_secs(15)))
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .build()
        .new_agent()
}

pub(super) fn json(agent: &Agent, url: &str, cancelled: &AtomicBool) -> Result<Value> {
    let mut response = checked(get(agent, url, "application/json", cancelled)?)?;
    let reader = body(&mut response);
    serde_json::from_reader(BufReader::new(reader)).map_err(|_| LookupError::Temporary(RETRY_DELAY))
}

pub(super) fn html(agent: &Agent, link: &PublicLink, cancelled: &AtomicBool) -> Result<String> {
    let mut url = Url::parse(&link.url).map_err(|_| LookupError::Unsupported)?;
    for _ in 0..=MAX_PUBLIC_REDIRECTS {
        let response = get(agent, url.as_str(), "text/html", cancelled)?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get("Location")
                .and_then(|value| value.to_str().ok());
            url = location
                .and_then(|location| checked_redirect(&url, location, link))
                .ok_or(LookupError::Missing)?;
            continue;
        }
        let mut response = checked(response)?;
        let mut text = String::new();
        body(&mut response)
            .read_to_string(&mut text)
            .map_err(|_| LookupError::Temporary(RETRY_DELAY))?;
        return Ok(text);
    }
    Err(LookupError::Missing)
}

fn checked_redirect(from: &Url, location: &str, link: &PublicLink) -> Option<Url> {
    let destination = from.join(location).ok()?;
    let same_storefront = link.platform != "apple-music"
        || from.path_segments()?.next()? == destination.path_segments()?.next()?;
    (from.origin() == destination.origin()
        && same_storefront
        && public_link(destination.as_str()).is_some_and(|target| link.same_address(&target)))
    .then_some(destination)
}

fn body(response: &mut Response<Body>) -> impl Read + '_ {
    telemetry::api_body(response.body_mut().with_config().limit(BODY_LIMIT).reader())
}

fn get(agent: &Agent, url: &str, accept: &str, cancelled: &AtomicBool) -> Result<Response<Body>> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(LookupError::Cancelled);
    }
    let response = agent
        .get(url)
        .header("Accept", accept)
        .header(
            "User-Agent",
            concat!("Mediadeka/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|_| LookupError::Temporary(RETRY_DELAY))?;
    if cancelled.load(Ordering::Relaxed) {
        return Err(LookupError::Cancelled);
    }
    #[cfg(test)]
    if matches!(response.status().as_u16(), 429 | 503) {
        eprintln!(
            "Public music metadata HTTP {} [{}]",
            response.status().as_u16(),
            Url::parse(url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
                .unwrap_or_default()
        );
    }
    Ok(response)
}

fn checked(response: Response<Body>) -> Result<Response<Body>> {
    if response.status().is_success() {
        return Ok(response);
    }
    match response.status().as_u16() {
        400 | 405 | 415 | 422 => Err(LookupError::Unsupported),
        429 | 503 => Err(LookupError::Temporary(retry_after(
            response
                .headers()
                .get("Retry-After")
                .and_then(|value| value.to_str().ok()),
            SystemTime::now(),
        ))),
        408 | 425 | 500..=599 => Err(LookupError::Temporary(RETRY_DELAY)),
        _ => Err(LookupError::Missing),
    }
}

fn retry_after(value: Option<&str>, now: SystemTime) -> Duration {
    value
        .map(str::trim)
        .and_then(|value| {
            value.parse().ok().map(Duration::from_secs).or_else(|| {
                httpdate::parse_http_date(value)
                    .ok()?
                    .duration_since(now)
                    .ok()
            })
        })
        .unwrap_or(RETRY_DELAY)
        .clamp(
            Duration::from_secs(1),
            Duration::from_secs(7 * 24 * 60 * 60),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honors_both_forms_of_retry_after() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let date = httpdate::fmt_http_date(now + Duration::from_secs(123));
        assert_eq!(retry_after(Some(&date), now), Duration::from_secs(123));
        assert_eq!(retry_after(Some("200"), now), Duration::from_secs(200));
        assert_eq!(retry_after(Some("invalid"), now), RETRY_DELAY);
    }

    #[test]
    fn catalog_redirects_must_keep_the_same_recording_and_storefront() {
        let link = public_link("https://music.apple.com/us/album/12?i=34").unwrap();
        let url = Url::parse(&link.url).unwrap();
        assert!(checked_redirect(&url, "/us/album/slug/12?i=34", &link).is_some());
        assert!(checked_redirect(&url, "/us/album/slug/12?i=56", &link).is_none());
        assert!(checked_redirect(&url, "/gb/album/slug/12?i=34", &link).is_none());
        assert!(checked_redirect(&url, "https://evil.invalid/us/album/12?i=34", &link).is_none());
    }
}
