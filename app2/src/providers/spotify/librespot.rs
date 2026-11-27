use std::{future::Future, pin::pin, sync::atomic::AtomicBool, time::Duration};

use librespot_core::{
    Error, Session, SessionConfig, authentication::Credentials, error::ErrorKind as ApErrorKind,
};
use librespot_protocol::authentication::AuthenticationType::AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS;
use serde_json::Value;
use tokio::{runtime::Builder, time::timeout};
use url::Url;

use crate::providers::{
    ErrorKind, Home, HomeSection, HomeSectionStyle, Profile, ProviderError,
    SpotifySessionCredentials,
    transport::{check_cancelled, field, valid_token},
};

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(20);
const PROFILE_TIMEOUT: Duration = Duration::from_secs(3);
const HOME_TIMEOUT: Duration = Duration::from_secs(22);
const LIBRARY_TIMEOUT: Duration = Duration::from_secs(5);
const CANCEL_CHECK: Duration = Duration::from_millis(100);

pub(super) fn connect(
    client_id: &str,
    access_token: Option<&str>,
    saved: Option<&SpotifySessionCredentials>,
    cancelled: Option<&AtomicBool>,
) -> Result<(Profile, SpotifySessionCredentials), ProviderError> {
    let runtime = runtime()?;
    runtime.block_on(async {
        let session = connect_ap(client_id, access_token, saved, cancelled).await?;
        let id = session.0.username();
        let name = profile_name(&session.0, &id, cancelled)
            .await?
            .unwrap_or_else(|| id.clone());
        check_if_cancelled(cancelled)?;
        let reusable = SpotifySessionCredentials {
            username: id.clone(),
            auth_data: session.0.auth_data(),
        };
        Ok((Profile { id, name }, reusable))
    })
}

pub(super) fn home(
    client_id: &str,
    saved: &SpotifySessionCredentials,
    cancelled: &AtomicBool,
) -> Result<Home, ProviderError> {
    let runtime = runtime()?;
    let result = runtime.block_on(async {
        let session = connect_ap(client_id, None, Some(saved), Some(cancelled)).await?;
        interruptible(home_with_library(&session.0), Some(cancelled)).await?
    });
    // A cancelled public metadata download may still be in ureq's bounded read.
    runtime.shutdown_background();
    result
}

async fn home_with_library(session: &Session) -> Result<Home, ProviderError> {
    let (recommendations, library) = tokio::join!(
        timeout(HOME_TIMEOUT, super::pathfinder::home(session)),
        timeout(
            LIBRARY_TIMEOUT,
            super::playlist::library_cards(session, Some(crate::providers::transport::HOME_LIMIT))
        ),
    );
    let recommendations = recommendations.unwrap_or_else(|_| {
        Err(ProviderError::new(
            ErrorKind::Network,
            "Spotify не ответил при загрузке рекомендаций.",
        ))
    });
    let Ok(Ok(cards)) = library else {
        return recommendations;
    };
    if cards.is_empty() {
        return recommendations;
    }
    let mut home = recommendations.unwrap_or_else(|_| Home {
        title: "Spotify".into(),
        items: Vec::new(),
        sections: Vec::new(),
    });
    home.sections.push(HomeSection {
        start: home.items.len(),
        len: cards.len(),
        title: "Плейлисты Spotify".into(),
        style: HomeSectionStyle::Collections,
        ..Default::default()
    });
    home.items.extend(cards);
    Ok(home)
}

pub(super) fn playlist(
    client_id: &str,
    saved: &SpotifySessionCredentials,
    id: &str,
    cancelled: &AtomicBool,
    progress: &mut crate::providers::playlists::PlaylistProgress<'_>,
) -> Result<crate::playlist::Playlist, ProviderError> {
    with_session(client_id, saved, cancelled, async |session| {
        super::playlist::load_with_progress(session, id, progress).await
    })
}

pub(super) fn with_session<T>(
    client_id: &str,
    saved: &SpotifySessionCredentials,
    cancelled: &AtomicBool,
    read: impl AsyncFnOnce(&Session) -> Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    let runtime = runtime()?;
    let result = runtime.block_on(async {
        let session = connect_ap(client_id, None, Some(saved), Some(cancelled)).await?;
        interruptible(read(&session.0), Some(cancelled)).await?
    });
    // A cancelled Pathfinder discovery may still be in its bounded blocking HTTP read.
    runtime.shutdown_background();
    result
}

fn runtime() -> Result<tokio::runtime::Runtime, ProviderError> {
    Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| {
            ProviderError::new(
                ErrorKind::Unavailable,
                "Не удалось запустить подключение Spotify.",
            )
        })
}

async fn connect_ap(
    client_id: &str,
    access_token: Option<&str>,
    saved: Option<&SpotifySessionCredentials>,
    cancelled: Option<&AtomicBool>,
) -> Result<ApSession, ProviderError> {
    check_if_cancelled(cancelled)?;
    let credentials = match saved {
        Some(saved) if valid_username(&saved.username) && !saved.auth_data.is_empty() => {
            Credentials {
                username: Some(saved.username.clone()),
                auth_type: AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS,
                auth_data: saved.auth_data.clone(),
            }
        }
        Some(_) => return Err(expired()),
        None => {
            let token = access_token
                .filter(|token| valid_token(token, 4096))
                .ok_or_else(expired)?;
            Credentials::with_access_token(token)
        }
    };
    let config = SessionConfig {
        client_id: client_id.to_owned(),
        ..Default::default()
    };
    #[cfg(test)]
    let config = SessionConfig {
        proxy: std::env::var("MEDIADEKA_TEST_SPOTIFY_PROXY")
            .ok()
            .and_then(|url| Url::parse(&url).ok()),
        ..config
    };
    let session = ApSession(Session::new(config, None));
    interruptible(
        timeout(CONNECTION_TIMEOUT, session.0.connect(credentials, false)),
        cancelled,
    )
    .await?
    .map_err(|_| {
        ProviderError::new(
            ErrorKind::Network,
            "Spotify не ответил при подтверждении профиля.",
        )
    })?
    .map_err(classify)?;

    let id = session.0.username();
    if !valid_username(&id) || session.0.auth_data().is_empty() {
        return Err(ProviderError::new(
            ErrorKind::InvalidResponse,
            "Spotify не подтвердил идентификатор профиля или данные для восстановления входа.",
        ));
    }
    if saved.is_some_and(|saved| saved.username != id) {
        return Err(ProviderError::new(
            ErrorKind::InvalidResponse,
            "Spotify вернул другой профиль. Подключите нужный аккаунт заново.",
        ));
    }
    Ok(session)
}

struct ApSession(Session);

impl Drop for ApSession {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

async fn profile_name(
    session: &Session,
    username: &str,
    cancelled: Option<&AtomicBool>,
) -> Result<Option<String>, ProviderError> {
    // Encode a single path component before handing it to spclient.
    let mut path = Url::parse("https://api.spotify.com/").expect("fixed Spotify origin");
    path.path_segments_mut()
        .expect("hierarchical HTTPS path")
        .push(username);
    let encoded = path.path().trim_start_matches('/');
    let response = interruptible(
        timeout(
            PROFILE_TIMEOUT,
            session
                .spclient()
                .get_user_profile(encoded, Some(0), Some(0)),
        ),
        cancelled,
    )
    .await?;
    let name = response.ok().and_then(Result::ok).and_then(|body| {
        let profile: Value = serde_json::from_slice(&body).ok()?;
        field(&profile, "display_name").or_else(|| field(&profile, "name"))
    });
    Ok(name)
}

async fn interruptible<F: Future>(
    future: F,
    cancelled: Option<&AtomicBool>,
) -> Result<F::Output, ProviderError> {
    let Some(cancelled) = cancelled else {
        return Ok(future.await);
    };
    let mut future = pin!(future);
    loop {
        check_cancelled(cancelled)?;
        if let Ok(value) = timeout(CANCEL_CHECK, future.as_mut()).await {
            check_cancelled(cancelled)?;
            return Ok(value);
        }
    }
}

fn check_if_cancelled(cancelled: Option<&AtomicBool>) -> Result<(), ProviderError> {
    cancelled.map_or(Ok(()), check_cancelled)
}

fn valid_username(username: &str) -> bool {
    !username.trim().is_empty() && username != "UNKNOWN" && !username.contains(char::is_control)
}

fn expired() -> ProviderError {
    ProviderError::new(
        ErrorKind::Expired,
        "Подтверждение Spotify истекло. Подключите аккаунт заново.",
    )
}

fn classify(error: Error) -> ProviderError {
    // The pinned access-point protocol reports all LoginFailed reasons as PermissionDenied.
    // Only invalid credentials should invalidate the stored authorization.
    let reason = (error.kind == ApErrorKind::PermissionDenied).then(|| error.error.to_string());
    let (kind, message) = match (error.kind, reason.as_deref()) {
        (ApErrorKind::Unauthenticated, _)
        | (_, Some("Login failed with reason: Bad credentials")) => {
            return expired();
        }
        (
            _,
            Some(
                "Login failed with reason: Could not validate credentials"
                | "Login failed with reason: Try another access point",
            ),
        ) => (
            ErrorKind::Network,
            "Spotify временно не смог подтвердить подключение.",
        ),
        (ApErrorKind::Cancelled, _) => (ErrorKind::Cancelled, "Вход отменён."),
        (ApErrorKind::PermissionDenied, _) => (
            ErrorKind::Rejected,
            "Spotify отклонил подтверждение профиля.",
        ),
        _ => (
            ErrorKind::Network,
            "Не удалось связаться со Spotify для подтверждения профиля.",
        ),
    };
    ProviderError::new(kind, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancels_an_in_flight_spotify_feed() {
        use std::{future::pending, sync::atomic::Ordering};

        let cancelled = AtomicBool::new(false);
        let result = runtime().unwrap().block_on(async {
            let request = interruptible(pending::<()>(), Some(&cancelled));
            let cancel = async {
                tokio::time::sleep(Duration::from_millis(1)).await;
                cancelled.store(true, Ordering::Relaxed);
            };
            tokio::join!(request, cancel).0
        });
        assert_eq!(result.unwrap_err().kind, ErrorKind::Cancelled);
    }

    #[test]
    fn bad_credentials_expire_but_service_and_account_refusals_stay_distinct() {
        let bad_credentials = Error::permission_denied("Login failed with reason: Bad credentials");
        let unavailable =
            Error::permission_denied("Login failed with reason: Could not validate credentials");
        let refused = Error::permission_denied("Login failed with reason: Travel restriction");
        assert_eq!(classify(bad_credentials).kind, ErrorKind::Expired);
        assert_eq!(classify(unavailable).kind, ErrorKind::Network);
        assert_eq!(classify(refused).kind, ErrorKind::Rejected);
    }
}
