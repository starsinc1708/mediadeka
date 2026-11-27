pub(super) mod catalog;
mod collection;
mod librespot;
pub(super) mod likes;
pub(super) mod lyrics;
mod metadata;
mod pathfinder;
mod playlist;
mod radio;
mod rate_limit;
pub(super) use collection::read_collection;
pub(crate) use radio::{artist_radio, track_radio};

use std::{
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

use super::{
    Credentials, ErrorKind, Home, LoginPrompt, Profile, ProviderError, ProviderId, Session,
    transport::*,
};
use rate_limit::{ACCOUNTS, LIBRARY, LoginRetry, Step};

const PROVIDER: ProviderId = ProviderId::Spotify;
const CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
const CALLBACK: &str = "http://127.0.0.1:8989/login";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const CALLBACK_LIMIT: u64 = 8192;
const SCOPES: &str = "streaming user-read-private user-read-email user-library-read user-library-modify user-follow-read";

#[derive(Deserialize)]
struct Token {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

pub fn sign_in(
    cancelled: &AtomicBool,
    prompt: impl Fn(LoginPrompt),
) -> Result<Session, ProviderError> {
    ACCOUNTS.check()?;
    let client_id =
        std::env::var("MEDIADEKA_SPOTIFY_CLIENT_ID").unwrap_or_else(|_| CLIENT_ID.into());
    let redirect =
        std::env::var("MEDIADEKA_SPOTIFY_REDIRECT_URI").unwrap_or_else(|_| CALLBACK.into());
    let callback = Url::parse(&redirect)
        .ok()
        .filter(|url| {
            url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && url.port().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        })
        .ok_or_else(|| {
            ProviderError::new(
                ErrorKind::InvalidInput,
                "Адрес возврата Spotify должен указывать на 127.0.0.1 и порт.",
            )
        })?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, callback.port().unwrap_or_default()))
        .and_then(|listener| {
            listener.set_nonblocking(true)?;
            Ok(listener)
        })
        .map_err(|_| {
            ProviderError::new(
                ErrorKind::Unavailable,
                "Порт возврата Spotify занят или недоступен.",
            )
        })?;
    let state = random()?;
    let verifier = random()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut authorize = Url::parse("https://accounts.spotify.com/authorize").expect("fixed URL");
    authorize.query_pairs_mut().extend_pairs([
        ("client_id", client_id.as_str()),
        ("redirect_uri", callback.as_str()),
        ("response_type", "code"),
        ("scope", SCOPES),
        ("state", &state),
        ("code_challenge_method", "S256"),
        ("code_challenge", &challenge),
    ]);
    prompt(LoginPrompt::Browser {
        url: authorize.into(),
    });
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    let code = loop {
        wait(cancelled, Duration::from_millis(50), deadline)?;
        match listener.accept() {
            Ok((mut connection, _)) => {
                let result = read_callback(&mut connection, &callback, &state);
                if let Some(result) = result {
                    break result?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => {
                return Err(ProviderError::new(
                    ErrorKind::Unavailable,
                    "Не удалось получить ответ браузера Spotify.",
                ));
            }
        }
    };
    check_cancelled(cancelled)?;
    let retry = LoginRetry {
        cancelled,
        prompt: &prompt,
    };
    let token = exchange(
        Step::ExchangeCode,
        Some(&retry),
        [
            ("client_id", client_id.as_str()),
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", callback.as_str()),
            ("code_verifier", &verifier),
        ],
    )?;
    let refresh_token = token
        .refresh_token
        .ok_or_else(|| invalid_response(PROVIDER))?;
    prompt(LoginPrompt::Waiting {
        message: "Подтверждаем подключение к Spotify…".into(),
    });
    let mut credentials = Credentials::Spotify {
        client_id,
        access_token: token.access_token,
        refresh_token,
        access_expires_at: expiration_after(token.expires_in),
        librespot: None,
    };
    let profile = confirm_session(&mut credentials, cancelled, false)?;
    Ok(Session::new(profile, credentials))
}

fn read_callback(
    stream: &mut TcpStream,
    callback: &Url,
    expected_state: &str,
) -> Option<Result<String, ProviderError>> {
    let deadline = Instant::now() + Duration::from_millis(700);
    stream
        .set_write_timeout(Some(Duration::from_millis(700)))
        .ok()?;
    let mut bytes = Vec::new();
    let mut limit = stream.take(CALLBACK_LIMIT);
    let mut chunk = [0; 256];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        limit.get_mut().set_read_timeout(Some(remaining)).ok()?;
        let count = limit.read(&mut chunk).ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let request = std::str::from_utf8(&bytes).ok()?;
    let target = request
        .lines()
        .next()?
        .strip_prefix("GET ")?
        .split_once(' ')?
        .0;
    let host = format!("127.0.0.1:{}", callback.port()?);
    if request.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("host").then(|| value.trim())
    }) != Some(host.as_str())
    {
        return None;
    }
    let result = parse_callback(target, callback, expected_state);
    let (status, text) = match &result {
        Some(Ok(_)) => ("200 OK", "Return to Mediadeka to finish Spotify sign-in."),
        Some(Err(_)) => ("200 OK", "Spotify sign-in stopped. Return to Mediadeka."),
        None => ("400 Bad Request", "Invalid sign-in request."),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    result
}

fn parse_callback(
    target: &str,
    callback: &Url,
    expected_state: &str,
) -> Option<Result<String, ProviderError>> {
    if !target.starts_with('/') || target.starts_with("//") {
        return None;
    }
    let url = callback.join(target).ok()?;
    if url.path() != callback.path() || url.origin() != callback.origin() {
        return None;
    }
    let mut query = url.query_pairs();
    if query.find(|(name, _)| name == "state")?.1 != expected_state {
        return None;
    }
    if let Some((_, error)) = url.query_pairs().find(|(name, _)| name == "error") {
        return Some(Err(if error == "access_denied" {
            ProviderError::new(ErrorKind::Cancelled, "Вход в Spotify отменён в браузере.")
        } else {
            ProviderError::new(ErrorKind::Rejected, "Spotify отклонил вход.")
        }));
    }
    Some(
        url.query_pairs()
            .find(|(name, value)| name == "code" && !value.is_empty())
            .map(|(_, code)| code.into_owned())
            .ok_or_else(|| invalid_response(PROVIDER)),
    )
}

fn exchange<const N: usize>(
    step: Step,
    retry: Option<&LoginRetry<'_>>,
    form: [(&str, &str); N],
) -> Result<Token, ProviderError> {
    let client = agent();
    let mut response = ACCOUNTS.call(step, retry, || client.post(TOKEN_URL).send_form(form))?;
    let status = response.status().as_u16();
    if status == 400 {
        let error: Value = decode_body(PROVIDER, &mut response)?;
        if error["error"] == "invalid_grant" {
            return Err(ProviderError::new(
                ErrorKind::Expired,
                "Подтверждение Spotify истекло. Подключите аккаунт заново.",
            ));
        }
    }
    decode(PROVIDER, Ok(response))
}

fn confirm_session(
    credentials: &mut Credentials,
    cancelled: &AtomicBool,
    use_access_token: bool,
) -> Result<Profile, ProviderError> {
    let Credentials::Spotify {
        client_id,
        access_token,
        librespot,
        ..
    } = credentials
    else {
        unreachable!()
    };
    let (profile, reusable) = self::librespot::connect(
        client_id,
        Some(access_token),
        if use_access_token {
            None
        } else {
            librespot.as_ref()
        },
        Some(cancelled),
    )?;
    *librespot = Some(reusable);
    Ok(profile)
}

pub fn restore(
    credentials: Credentials,
    previous_profile: &Profile,
    cancelled: &AtomicBool,
) -> Result<Session, ProviderError> {
    restore_with(
        credentials,
        previous_profile,
        cancelled,
        confirm_session,
        refresh_access_token,
    )
}

fn restore_with(
    mut credentials: Credentials,
    previous_profile: &Profile,
    cancelled: &AtomicBool,
    mut confirm: impl FnMut(&mut Credentials, &AtomicBool, bool) -> Result<Profile, ProviderError>,
    refresh: impl FnOnce(&mut Credentials) -> Result<(), ProviderError>,
) -> Result<Session, ProviderError> {
    check_cancelled(cancelled)?;
    let Credentials::Spotify { librespot, .. } = &credentials else {
        unreachable!()
    };
    if librespot.is_none() {
        // Keep the known profile visible for saved grants from before librespot.
        // The home result guides the user to reconnect it with streaming access.
        return Ok(Session::new(previous_profile.clone(), credentials));
    }

    let mut profile = match confirm(&mut credentials, cancelled, false) {
        Ok(profile) => profile,
        Err(error) if error.kind == ErrorKind::Expired => {
            let current = has_current_access_token(&credentials)
                .then(|| confirm(&mut credentials, cancelled, true));
            match current {
                Some(Ok(profile)) => profile,
                None
                | Some(Err(ProviderError {
                    kind: ErrorKind::Expired,
                    ..
                })) => {
                    check_cancelled(cancelled)?;
                    refresh(&mut credentials)?;
                    // Finish the bounded check so the caller can save the rotated grant.
                    confirm(&mut credentials, &AtomicBool::new(false), true)?
                }
                Some(Err(error)) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    };
    if profile.id == previous_profile.id && profile.name == profile.id {
        profile.name.clone_from(&previous_profile.name);
    }
    Ok(Session::new(profile, credentials))
}

pub(crate) fn refresh_library_access(
    credentials: &mut Credentials,
    profile: &Profile,
    cancelled: &AtomicBool,
) -> Result<bool, ProviderError> {
    library_grant(credentials, profile, cancelled)?;
    if has_current_access_token(credentials) {
        return Ok(false);
    }
    refresh_access_token(credentials)?;
    // A successful exchange may have rotated the refresh token. The caller
    // must persist it before honoring cancellation of the library operation.
    Ok(true)
}

fn library_access_token<'a>(
    credentials: &'a Credentials,
    profile: &Profile,
    cancelled: &AtomicBool,
) -> Result<&'a str, ProviderError> {
    let token = library_grant(credentials, profile, cancelled)?;
    if !valid_token(token, 4096) {
        return Err(invalid_response(PROVIDER));
    }
    Ok(token)
}

fn library_grant<'a>(
    credentials: &'a Credentials,
    profile: &Profile,
    cancelled: &AtomicBool,
) -> Result<&'a str, ProviderError> {
    check_cancelled(cancelled)?;
    let Credentials::Spotify {
        access_token,
        librespot: Some(saved),
        ..
    } = credentials
    else {
        return Err(missing_library_profile());
    };
    if saved.username != profile.id {
        return Err(missing_library_profile());
    }
    LIBRARY.check()?;
    Ok(access_token)
}

fn missing_library_profile() -> ProviderError {
    ProviderError::new(
        ErrorKind::Expired,
        "Подключите нужный профиль Spotify заново.",
    )
}

fn has_current_access_token(credentials: &Credentials) -> bool {
    let Credentials::Spotify {
        access_token,
        access_expires_at,
        ..
    } = credentials
    else {
        unreachable!()
    };
    valid_token(access_token, 4096)
        && access_expires_at
            .zip(epoch_seconds())
            .is_some_and(|(end, now)| end > now)
}

pub(super) fn refresh_access_token(credentials: &mut Credentials) -> Result<(), ProviderError> {
    let Credentials::Spotify {
        client_id,
        access_token,
        refresh_token,
        access_expires_at,
        ..
    } = credentials
    else {
        unreachable!()
    };
    if !valid_token(refresh_token, 4096) {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "Подключение Spotify истекло. Войдите снова.",
        ));
    }
    let token = exchange(
        Step::RefreshToken,
        None,
        [
            ("client_id", client_id.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
        ],
    )?;
    if !valid_token(&token.access_token, 4096)
        || token
            .refresh_token
            .as_deref()
            .is_some_and(|rotated| !valid_token(rotated, 4096))
    {
        return Err(invalid_response(PROVIDER));
    }
    *access_token = token.access_token;
    if let Some(rotated) = token.refresh_token {
        *refresh_token = rotated;
    }
    *access_expires_at = expiration_after(token.expires_in);
    Ok(())
}

pub fn home(session: &Session, cancelled: &AtomicBool) -> Result<Home, ProviderError> {
    let Credentials::Spotify {
        client_id,
        librespot,
        ..
    } = &session.credentials
    else {
        unreachable!()
    };
    let saved = librespot.as_ref().ok_or_else(|| {
        ProviderError::new(
            ErrorKind::Expired,
            "Подключите Spotify заново, чтобы получить персональные рекомендации.",
        )
    })?;
    if saved.username != session.profile.id {
        return Err(invalid_response(PROVIDER));
    }
    rate_limit::FEED.check()?;
    self::librespot::home(client_id, saved, cancelled)
}

pub(super) fn playlist(
    client_id: &str,
    saved: Option<&super::SpotifySessionCredentials>,
    id: &str,
    cancelled: &AtomicBool,
    progress: &mut super::playlists::PlaylistProgress<'_>,
) -> Result<crate::playlist::Playlist, ProviderError> {
    let saved = saved.ok_or_else(|| {
        ProviderError::new(
            ErrorKind::Expired,
            "Подключите Spotify заново, чтобы открывать плейлисты.",
        )
    })?;
    self::librespot::playlist(client_id, saved, id, cancelled, progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_callback_with_wrong_state_or_path() {
        let callback = Url::parse(CALLBACK).unwrap();
        assert!(parse_callback("/login?state=wrong&code=abc", &callback, "expected").is_none());
        assert!(parse_callback("/other?state=expected&code=abc", &callback, "expected").is_none());
        assert_eq!(
            parse_callback("/login?code=abc&state=expected", &callback, "expected"),
            Some(Ok("abc".into()))
        );
    }

    #[test]
    fn spotify_credentials_can_read_the_previous_storage_format() {
        let credentials: Credentials = serde_json::from_str(
            r#"{"provider":"spotify","client_id":"client","access_token":"access","refresh_token":"refresh"}"#,
        )
        .unwrap();
        assert!(matches!(
            &credentials,
            Credentials::Spotify {
                librespot: None,
                access_expires_at: None,
                ..
            }
        ));
        assert!(!has_current_access_token(&credentials));
    }

    #[test]
    fn rejected_current_access_is_refreshed_after_stored_session_expires() {
        let profile = Profile {
            id: "account".into(),
            name: "Account".into(),
        };
        let credentials = Credentials::Spotify {
            client_id: "client".into(),
            access_token: "old-access".into(),
            refresh_token: "refresh".into(),
            access_expires_at: Some(u64::MAX),
            librespot: Some(super::super::SpotifySessionCredentials {
                username: profile.id.clone(),
                auth_data: vec![1],
            }),
        };
        let mut attempts = Vec::new();
        let restored = restore_with(
            credentials,
            &profile,
            &AtomicBool::new(false),
            |credentials, _, use_token| {
                let Credentials::Spotify { access_token, .. } = credentials else {
                    unreachable!()
                };
                attempts.push((use_token, access_token.clone()));
                if access_token == "new-access" {
                    Ok(profile.clone())
                } else {
                    Err(ProviderError::new(
                        ErrorKind::Expired,
                        "test credentials expired",
                    ))
                }
            },
            |credentials| {
                let Credentials::Spotify {
                    access_token,
                    refresh_token,
                    ..
                } = credentials
                else {
                    unreachable!()
                };
                *access_token = "new-access".into();
                *refresh_token = "rotated".into();
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            attempts,
            [
                (false, "old-access".into()),
                (true, "old-access".into()),
                (true, "new-access".into())
            ]
        );
        assert!(
            matches!(restored.credentials, Credentials::Spotify { refresh_token, .. } if refresh_token == "rotated")
        );
        assert_eq!(restored.profile, profile);
    }
}
