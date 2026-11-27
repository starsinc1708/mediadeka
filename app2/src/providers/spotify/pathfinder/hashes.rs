use std::{
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

use serde_json::Value;
use ureq::Agent;

use super::Operation;

const SPOTIFY_WEB: &str = "https://open.spotify.com/";
const DESKTOP_BUNDLE: &str = "https://open.spotifycdn.com/cdn/build/web-player/web-player.";
// Browse and search live outside the main desktop bundle.
const QUERY_REGISTRY: &str = "https://billowing-resonance-da83.johnwatson.workers.dev/hashes";
const DESKTOP_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";
const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const PAGE_LIMIT: u64 = 1024 * 1024;
const BUNDLE_LIMIT: u64 = 12 * 1024 * 1024;

static HOME: Mutex<Option<Discovered>> = Mutex::new(None);
static BROWSE: Mutex<Option<Discovered>> = Mutex::new(None);
static SEARCH: Mutex<Option<Discovered>> = Mutex::new(None);
static ARTIST: Mutex<Option<Discovered>> = Mutex::new(None);

#[derive(Clone)]
struct Discovered {
    when: Instant,
    value: String,
}

pub(super) async fn get(operation: Operation, rejected: Option<String>) -> Option<String> {
    let slot = match operation {
        Operation::Home => &HOME,
        Operation::BrowsePage => &BROWSE,
        Operation::Search => &SEARCH,
        Operation::Artist => &ARTIST,
    };
    let cached = slot.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let reusable = cached.filter(|hash| rejected.as_ref() != Some(&hash.value));
    if let Some(hash) = reusable
        .as_ref()
        .filter(|hash| hash.when.elapsed() < MAX_AGE)
    {
        return Some(hash.value.clone());
    }

    // Discovery reads public assets only. No Spotify authorization enters this client.
    if let Some(value) = tokio::task::spawn_blocking(move || discover(operation))
        .await
        .ok()
        .flatten()
        .filter(|value| rejected.as_ref() != Some(value))
    {
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(Discovered {
            when: Instant::now(),
            value: value.clone(),
        });
        return Some(value);
    }
    reusable.map(|hash| hash.value)
}

fn discover(operation: Operation) -> Option<String> {
    let agent = crate::providers::transport::agent();
    match operation {
        Operation::Home => {
            let page = public_text(&agent, SPOTIFY_WEB, PAGE_LIMIT)?;
            let script = public_text(&agent, desktop_bundle(&page)?, BUNDLE_LIMIT)?;
            query_hash(&script, operation.name()).map(str::to_owned)
        }
        Operation::BrowsePage | Operation::Search | Operation::Artist => {
            let registry = public_text(&agent, QUERY_REGISTRY, PAGE_LIMIT)?;
            let value: Value = serde_json::from_str(&registry).ok()?;
            value["operations"][operation.name()]
                .as_str()
                .filter(|hash| valid_digest(hash))
                .map(str::to_owned)
        }
    }
}

fn public_text(agent: &Agent, url: &str, limit: u64) -> Option<String> {
    let mut response = agent
        .get(url)
        .header("User-Agent", DESKTOP_USER_AGENT)
        .call()
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_string()
        .ok()
}

fn desktop_bundle(html: &str) -> Option<&str> {
    html.split('"').find(|url| {
        url.strip_prefix(DESKTOP_BUNDLE)
            .and_then(|name| name.strip_suffix(".js"))
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
    })
}

fn query_hash<'a>(script: &'a str, operation: &str) -> Option<&'a str> {
    let marker = format!("\"{operation}\",\"query\",\"");
    let (_, tail) = script.split_once(&marker)?;
    let (hash, _) = tail.split_once('"')?;
    valid_digest(hash).then_some(hash)
}

fn valid_digest(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_https_desktop_bundle_and_valid_persisted_query() {
        let url = format!("{DESKTOP_BUNDLE}abcdef.js");
        let html = format!(
            r#"<script src="https://example.org/web-player.abc.js"></script><script src="{url}"></script>"#
        );
        assert_eq!(desktop_bundle(&html), Some(url.as_str()));
        assert!(desktop_bundle(r#"<script src="https://open.spotifycdn.com/cdn/build/web-player/web-player.bad.js/other.js"></script>"#).is_none());
        let digest = "9f".repeat(32);
        assert_eq!(
            query_hash(
                &format!(r#"new Query("home","query","{digest}",null)"#),
                "home"
            ),
            Some(digest.as_str())
        );
        assert!(query_hash(r#"new Query("home","query","no")"#, "home").is_none());
    }

    #[test]
    #[ignore = "reads the public Spotify desktop bundle and query registry"]
    fn live_discovers_spotify_home_query_digests() {
        for operation in [
            Operation::Home,
            Operation::BrowsePage,
            Operation::Search,
            Operation::Artist,
        ] {
            assert!(discover(operation).is_some(), "{}", operation.name());
        }
    }
}
