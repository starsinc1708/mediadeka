

mod feed;
mod hashes;

use std::time::Duration;

use http_body_util::{BodyExt, Limited};
use librespot_core::{Error, Session, error::ErrorKind as SpotifyError, spclient::CLIENT_TOKEN};
use serde_json::{Value, json};
use tokio::time::timeout;
use ureq::http::{Request, StatusCode, header};

use crate::providers::{ErrorKind, Home, ProviderError};

use super::rate_limit::{FEED, Step};

const ENDPOINT: &str = "https://api-partner.spotify.com/pathfinder/v2/query";
const INTEGRATION: &str = "INTEGRATION_WEB_PLAYER";
const MADE_FOR_YOU: &str = "spotify:page:0JQ5DAt0tbjZptfcdMSKl3";
const APP_VERSION: &str = "896000000";
const ITEMS_PER_SHELF: usize = 10;
const MADE_FOR_YOU_SHELVES: usize = 20;
const QUERY_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy)]
enum Operation {
    Home,
    BrowsePage,
    Search,
    Artist,
}

impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::BrowsePage => "browsePage",
            Self::Search => "searchDesktop",
            Self::Artist => "queryArtistOverview",
        }
    }

    fn variables(self, argument: &str) -> Value {
        match self {
            Self::Home => json!({
                "homeEndUserIntegration": INTEGRATION,
                "timeZone": "UTC",
                "sp_t": "",
                "facet": "",
                "sectionItemsLimit": ITEMS_PER_SHELF,
                "includeEpisodeContentRatingsV2": false,
            }),
            Self::BrowsePage => json!({
                "uri": argument,
                "pagePagination": { "offset": 0, "limit": MADE_FOR_YOU_SHELVES },
                "sectionPagination": { "offset": 0, "limit": ITEMS_PER_SHELF },
                "browseEndUserIntegration": INTEGRATION,
                "includeEpisodeContentRatingsV2": false,
            }),
            Self::Search => json!({
                "searchTerm": argument,
                "offset": 0,
                "limit": crate::providers::catalog::SEARCH_LIMIT,
                "numberOfTopResults": 1,
                "includeAudiobooks": false,
                "includeArtistHasConcertsField": false,
                "includePreReleases": false,
                "includeAlbumPreReleases": false,
                "includeAuthors": false,
                "includeEpisodeContentRatingsV2": false,
            }),
            Self::Artist => json!({
                "uri": format!("spotify:artist:{argument}"),
                "locale": "",
                "preReleaseV2": true,
            }),
        }
    }
}

pub(super) async fn home(session: &Session) -> Result<Home, ProviderError> {
    let token = session.login5().auth_token().await.map_err(service_error)?;
    let client_token = session
        .spclient()
        .client_token()
        .await
        .map_err(service_error)?;
    let authorization = format!("{} {}", token.token_type, token.access_token);
    let load = |operation, argument| {
        timeout(
            QUERY_TIMEOUT,
            query(session, operation, argument, &authorization, &client_token),
        )
    };
    let (home, made) = tokio::join!(
        load(Operation::Home, ""),
        load(Operation::BrowsePage, MADE_FOR_YOU)
    );
    let home = home.unwrap_or_else(|_| Err(unavailable()));
    let made = made.unwrap_or_else(|_| Err(unavailable()));
    match (home, made) {
        (Err(home), Err(_)) => FEED.check().and(Err(home)),
        (home, made) => {
            feed::compose(home.ok().as_ref(), made.ok().as_ref()).ok_or_else(unavailable)
        }
    }
}

pub(super) async fn search(session: &Session, term: &str) -> Result<Value, ProviderError> {
    catalog_query(session, Operation::Search, term).await
}

pub(super) async fn artist(session: &Session, id: &str) -> Result<Value, ProviderError> {
    catalog_query(session, Operation::Artist, id).await
}

pub(super) async fn collection(session: &Session, id: &str) -> Result<Home, ProviderError> {
    let uri = format!("spotify:page:{id}");
    let data = catalog_query(session, Operation::BrowsePage, &uri).await?;
    feed::collection(&data).ok_or_else(unavailable)
}

async fn catalog_query(
    session: &Session,
    operation: Operation,
    argument: &str,
) -> Result<Value, ProviderError> {
    timeout(QUERY_TIMEOUT, async {
        let token = session.login5().auth_token().await.map_err(service_error)?;
        let client_token = session
            .spclient()
            .client_token()
            .await
            .map_err(service_error)?;
        let authorization = format!("{} {}", token.token_type, token.access_token);
        query(session, operation, argument, &authorization, &client_token).await
    })
    .await
    .map_err(|_| ProviderError::new(ErrorKind::Network, "Spotify не ответил на запрос каталога."))?
}

async fn query(
    session: &Session,
    operation: Operation,
    argument: &str,
    authorization: &str,
    client_token: &str,
) -> Result<Value, ProviderError> {
    let hash = hashes::get(operation, None).await.ok_or_else(unavailable)?;
    match send(
        session,
        operation,
        argument,
        &hash,
        authorization,
        client_token,
    )
    .await?
    {
        Some(data) => Ok(data),
        None => {
            let updated = hashes::get(operation, Some(hash))
                .await
                .ok_or_else(unavailable)?;
            send(
                session,
                operation,
                argument,
                &updated,
                authorization,
                client_token,
            )
            .await?
            .ok_or_else(unavailable)
        }
    }
}

async fn send(
    session: &Session,
    operation: Operation,
    argument: &str,
    hash: &str,
    authorization: &str,
    client_token: &str,
) -> Result<Option<Value>, ProviderError> {
    FEED.check()?;
    let body = serde_json::to_vec(&json!({
        "operationName": operation.name(),
        "variables": operation.variables(argument),
        "extensions": {
            "persistedQuery": {
                "version": 1,
                "sha256Hash": hash,
            },
        },
    }))
    .map_err(|_| unavailable())?;
    let request = Request::post(ENDPOINT)
        .header(header::ACCEPT, "application/json")
        .header(header::CONTENT_TYPE, "application/json")
        .header("app-platform", "WebPlayer")
        .header("spotify-app-version", APP_VERSION)
        .header(header::AUTHORIZATION, authorization)
        .header(CLIENT_TOKEN, client_token)
        .body(body.into())
        .map_err(|_| unavailable())?;
    // This sends one request. The automatic Retry-After loop in request_body
    // would hide a long cooldown and could repeat a short one many times.
    let response = session
        .http_client()
        .request_fut(request)
        .map_err(service_error)?
        .await
        .map_err(|error| service_error(error.into()))?;
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        return Err(FEED.limited(Step::Feed, response.headers()));
    }
    if !response.status().is_success() {
        return Err(unavailable());
    }
    let body = Limited::new(response.into_body(), RESPONSE_LIMIT)
        .collect()
        .await
        .map_err(|_| unavailable())?
        .to_bytes();
    let mut response: Value = serde_json::from_slice(&body).map_err(|_| unavailable())?;
    let data = response["data"].take();
    if !data.is_null() {
        return Ok(Some(data));
    }
    if response["errors"].as_array().is_some_and(|errors| {
        errors.iter().any(|error| {
            error["message"]
                .as_str()
                .is_some_and(|message| message.to_ascii_lowercase().contains("persisted"))
        })
    }) {
        return Ok(None);
    }
    Err(unavailable())
}

fn service_error(error: Error) -> ProviderError {
    if error.kind == SpotifyError::ResourceExhausted {
        ProviderError::new(
            ErrorKind::Network,
            "Spotify временно ограничил загрузку рекомендаций. Повторите позже.",
        )
    } else {
        unavailable()
    }
}

fn unavailable() -> ProviderError {
    ProviderError::new(
        ErrorKind::Unavailable,
        "Не удалось загрузить рекомендации Spotify.",
    )
}
