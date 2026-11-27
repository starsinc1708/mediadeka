#[cfg(test)]
mod tests;

use std::{
    path::Path,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use serde_json::Value;
use url::Url;

use super::{PROVIDER, TOKEN_LIMIT, get_with, valid_client_id};
use crate::providers::{
    Credentials, ErrorKind, LoginPrompt, Profile, ProviderError, Session,
    transport::{
        agent, check_cancelled, field, invalid_response, network, response_error, valid_token,
    },
};

const PUBLIC_PAGE: &str = "https://soundcloud.com/";
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(18);
const MAX_SCRIPTS: usize = 12;
const RESPONSE_LIMIT: u64 = 4 * 1024 * 1024;

pub(in crate::providers) fn browser_sign_in(
    owner: isize,
    cancelled: &AtomicBool,
    prompt: impl Fn(LoginPrompt),
) -> Result<Session, ProviderError> {
    #[cfg(windows)]
    {
        prompt(LoginPrompt::Waiting {
            message: "Войдите в SoundCloud в отдельном окне Mediadeka.".into(),
        });
        let profile = crate::storage::Store::profile_directory().map_err(|_| {
            ProviderError::new(
                ErrorKind::Unavailable,
                "Не удалось определить каталог профиля для входа в SoundCloud.",
            )
        })?;
        super::browser::sign_in(&profile, owner, cancelled, |token| {
            prompt(LoginPrompt::Waiting {
                message: "Проверяем аккаунт SoundCloud…".into(),
            });
            sign_in(token, cancelled)
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (owner, prompt);
        check_cancelled(cancelled)?;
        Err(ProviderError::new(
            ErrorKind::Unavailable,
            "Для встроенного входа в SoundCloud требуется Windows.",
        ))
    }
}

#[cfg(windows)]
pub(in crate::providers) fn sign_in(
    token: String,
    cancelled: &AtomicBool,
) -> Result<Session, ProviderError> {
    check_cancelled(cancelled)?;
    if !valid_token(&token, TOKEN_LIMIT) {
        return Err(ProviderError::new(
            ErrorKind::InvalidResponse,
            "SoundCloud вернул некорректный сеанс входа.",
        ));
    }
    let client_id = discover_client_id(cancelled)?;
    check_cancelled(cancelled)?;
    let response = get_with(&agent(), "/me", &token, &client_id);
    check_cancelled(cancelled)?;
    profile(response.map_err(profile_error)?, token, client_id).map_err(profile_error)
}

pub(in crate::providers) fn restore(
    credentials: Credentials,
    cancelled: &AtomicBool,
) -> Result<Session, ProviderError> {
    restore_credentials(credentials, cancelled, |_| Ok(None))
}

pub(in crate::providers) fn restore_in_profile(
    credentials: Credentials,
    previous: &Profile,
    directory: &Path,
    cancelled: &AtomicBool,
) -> Result<Session, ProviderError> {
    let session = restore_credentials(credentials, cancelled, |expired| {
        #[cfg(windows)]
        return super::browser::renew(directory, expired, cancelled);
        #[cfg(not(windows))]
        {
            let _ = (directory, expired);
            Ok(None)
        }
    })?;
    confirm_restored_profile(session, previous)
}

fn confirm_restored_profile(
    session: Session,
    previous: &Profile,
) -> Result<Session, ProviderError> {
    if session.profile.id != previous.id {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "SoundCloud вернул другой профиль. Войдите снова в сохранённый аккаунт.",
        ));
    }
    Ok(session)
}

pub(crate) fn clear_browser_session(
    directory: &Path,
    cancelled: &AtomicBool,
) -> Result<(), ProviderError> {
    #[cfg(windows)]
    return super::browser::clear_browser_session(directory, cancelled);
    #[cfg(not(windows))]
    {
        let _ = directory;
        check_cancelled(cancelled)
    }
}

fn restore_credentials(
    credentials: Credentials,
    cancelled: &AtomicBool,
    renew: impl FnOnce(&str) -> Result<Option<String>, ProviderError>,
) -> Result<Session, ProviderError> {
    let agent = agent();
    restore_with(
        credentials,
        cancelled,
        |token, client_id| get_with(&agent, "/me", token, client_id),
        || discover_client_id(cancelled),
        renew,
    )
}

fn restore_with(
    credentials: Credentials,
    cancelled: &AtomicBool,
    mut verify: impl FnMut(&str, &str) -> Result<Value, ProviderError>,
    discover: impl FnOnce() -> Result<String, ProviderError>,
    renew: impl FnOnce(&str) -> Result<Option<String>, ProviderError>,
) -> Result<Session, ProviderError> {
    let Credentials::SoundCloud {
        mut token,
        mut client_id,
    } = credentials
    else {
        unreachable!()
    };
    check_cancelled(cancelled)?;
    if !valid_token(&token, TOKEN_LIMIT) {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "Данные сеанса SoundCloud недействительны. Войдите снова.",
        ));
    }

    let mut response = verify(&token, &client_id);
    check_cancelled(cancelled)?;
    if response
        .as_ref()
        .is_err_and(|error| matches!(error.kind, ErrorKind::Rejected | ErrorKind::Expired))
    {
        let discovered = discover();
        check_cancelled(cancelled)?;
        let current_client = discovered?;
        if current_client != client_id {
            response = verify(&token, &current_client);
            check_cancelled(cancelled)?;
            client_id = current_client;
        }
    }
    if response
        .as_ref()
        .is_err_and(|error| error.kind == ErrorKind::Expired)
    {
        let renewed = renew(&token);
        check_cancelled(cancelled)?;
        if let Some(new_token) = renewed?.filter(|new_token| new_token != &token) {
            response = verify(&new_token, &client_id);
            check_cancelled(cancelled)?;
            token = new_token;
        }
    }
    profile(response.map_err(profile_error)?, token, client_id).map_err(profile_error)
}

fn profile_error(error: ProviderError) -> ProviderError {
    match error.kind {
        ErrorKind::Rejected => ProviderError::new(
            ErrorKind::Unavailable,
            "SoundCloud не подтвердил профиль (HTTP 403). Повторите проверку.",
        ),
        ErrorKind::InvalidResponse => ProviderError::new(
            ErrorKind::Unavailable,
            "SoundCloud временно не вернул данные профиля. Повторите проверку.",
        ),
        _ => error,
    }
}

fn profile(response: Value, token: String, client_id: String) -> Result<Session, ProviderError> {
    let id = response["id"]
        .as_u64()
        .map(|id| id.to_string())
        .ok_or_else(|| invalid_response(PROVIDER))?;
    let name = field(&response, "username").ok_or_else(|| invalid_response(PROVIDER))?;
    Ok(Session::new(
        Profile { id, name },
        Credentials::SoundCloud { token, client_id },
    ))
}

fn discover_client_id(cancelled: &AtomicBool) -> Result<String, ProviderError> {
    check_cancelled(cancelled)?;
    let deadline = Instant::now() + DISCOVERY_TIMEOUT;
    let agent = agent();
    let page = fetch_text(&agent, PUBLIC_PAGE)?;
    check_cancelled(cancelled)?;
    if let Some(id) = hydrated_client_id(&page) {
        return Ok(id);
    }
    for url in script_urls(&page).into_iter().rev().take(MAX_SCRIPTS) {
        check_cancelled(cancelled)?;
        if Instant::now() >= deadline {
            break;
        }
        if let Ok(script) = fetch_text(&agent, url.as_str())
            && let Some(id) = inline_client_id(&script)
        {
            return Ok(id.to_owned());
        }
    }
    Err(configuration_unavailable())
}

fn hydrated_client_id(page: &str) -> Option<String> {
    page.split("window.__sc_hydration")
        .skip(1)
        .find_map(|assignment| {
            let data = assignment.trim_start().strip_prefix('=')?.trim_start();
            let entries = serde_json::Deserializer::from_str(data)
                .into_iter::<Vec<Value>>()
                .next()?
                .ok()?;
            let client = entries
                .iter()
                .find(|entry| entry["hydratable"] == "apiClient")?;
            let id = client["data"]["id"].as_str()?;
            valid_client_id(id).then(|| id.to_owned())
        })
}

fn script_urls(page: &str) -> Vec<Url> {
    page.split("<script")
        .skip(1)
        .filter_map(|tag| {
            let (attributes, _) = tag.split_once('>')?;
            let (_, value) = attributes.split_once(" src=")?;
            let quote = value
                .chars()
                .next()
                .filter(|quote| matches!(quote, '"' | '\''))?;
            let url = Url::parse(value.get(1..)?.split_once(quote)?.0).ok()?;
            let host = url.host_str()?;
            (url.scheme() == "https"
                && host.ends_with(".sndcdn.com")
                && url.path().ends_with(".js")
                && url.port().is_none()
                && url.username().is_empty()
                && url.password().is_none())
            .then_some(url)
        })
        .collect()
}

fn inline_client_id(script: &str) -> Option<&str> {
    script.match_indices("client_id").find_map(|(index, name)| {
        let value = script
            .get(index + name.len()..)?
            .trim_start()
            .strip_prefix(':')?
            .trim_start();
        let quote = value
            .chars()
            .next()
            .filter(|quote| matches!(quote, '"' | '\''))?;
        let id = value.get(1..)?.split_once(quote)?.0;
        valid_client_id(id).then_some(id)
    })
}

fn fetch_text(agent: &ureq::Agent, url: &str) -> Result<String, ProviderError> {
    let mut response = agent
        .get(url)
        .call()
        .map_err(|error| network(PROVIDER, error))?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let mut error = response_error(PROVIDER, status);
        if matches!(error.kind, ErrorKind::Expired | ErrorKind::Rejected) {
            error.kind = ErrorKind::Unavailable;
        }
        return Err(error);
    }
    response
        .body_mut()
        .with_config()
        .limit(RESPONSE_LIMIT)
        .read_to_string()
        .map_err(|_| configuration_unavailable())
}

fn configuration_unavailable() -> ProviderError {
    ProviderError::new(
        ErrorKind::Unavailable,
        "Не удалось получить публичную конфигурацию SoundCloud.",
    )
}
