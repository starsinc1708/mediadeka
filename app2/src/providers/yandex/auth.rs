use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use serde_json::Value;
use url::Url;

use super::{
    super::{Credentials, ErrorKind, LoginPrompt, Profile, ProviderError, Session, transport::*},
    PROVIDER, TOKEN_LIMIT, endpoint, get, numeric_id,
};

const OAUTH: &str = "https://oauth.yandex.ru";
const CLIENT_ID: &str = "23cabbbdc6cd418abb4b39c32c41195d";
const CLIENT_SECRET: &str = "53bc75238f0c4d08a118e51fe9203300";
const TOKEN_PATH: &str = "/token";
const VERIFICATION_URL: &str = "https://oauth.yandex.ru/device";

struct Grant {
    access_token: String,
    refresh_token: Option<String>,
    access_expires_at: Option<u64>,
}

pub(in crate::providers) fn sign_in(
    cancelled: &AtomicBool,
    prompt: impl Fn(LoginPrompt),
) -> Result<Session, ProviderError> {
    let device = random()?;
    let agent = agent();
    let data: Value = decode(
        PROVIDER,
        agent.post(&endpoint(OAUTH, "/device/code")).send_form([
            ("client_id", CLIENT_ID),
            ("device_id", &device),
            ("device_name", "Mediadeka"),
        ]),
    )?;
    let code = field(&data, "user_code").ok_or_else(|| invalid_response(PROVIDER))?;
    let device_code = field(&data, "device_code").ok_or_else(|| invalid_response(PROVIDER))?;
    let url = data["verification_url"]
        .as_str()
        .and_then(verification_url)
        .unwrap_or_else(|| VERIFICATION_URL.into());
    check_cancelled(cancelled)?;
    prompt(LoginPrompt::Code { code, url });
    let mut interval = Duration::from_secs(data["interval"].as_u64().unwrap_or(5).clamp(1, 60));
    let deadline = Instant::now()
        + Duration::from_secs(data["expires_in"].as_u64().unwrap_or(300).clamp(1, 1800));
    loop {
        wait(cancelled, interval, deadline)?;
        let (status, token) = exchange(&agent, "device_code", ("code", &device_code))?;
        if status == 200 {
            let Grant {
                access_token,
                refresh_token,
                access_expires_at,
            } = grant(&token)?;
            check_cancelled(cancelled)?;
            let profile = confirm_profile(&agent, &access_token)?;
            check_cancelled(cancelled)?;
            return Ok(Session::new(
                profile,
                Credentials::YandexMusic {
                    access_token,
                    refresh_token,
                    access_expires_at,
                },
            ));
        }
        match token["error"].as_str() {
            Some("authorization_pending") => continue,
            Some("slow_down") => interval += Duration::from_secs(5),
            Some("expired_token" | "invalid_grant") => {
                return Err(ProviderError::new(
                    ErrorKind::Expired,
                    "Код Яндекса истёк. Получите новый код.",
                ));
            }
            Some("access_denied" | "authorization_declined") => {
                return Err(ProviderError::new(
                    ErrorKind::Cancelled,
                    "Вход в Яндекс Музыку отменён в браузере.",
                ));
            }
            _ => return Err(response_error(PROVIDER, status)),
        }
    }
}

pub(in crate::providers) fn restore(
    credentials: Credentials,
    cancelled: &AtomicBool,
) -> Result<Session, ProviderError> {
    check_cancelled(cancelled)?;
    let Credentials::YandexMusic { access_token, .. } = &credentials else {
        unreachable!()
    };
    let agent = agent();
    let profile = confirm_profile(&agent, access_token);
    check_cancelled(cancelled)?;
    match profile {
        Ok(profile) => Ok(Session::new(profile, credentials)),
        Err(error) if error.kind == ErrorKind::Expired => renew_with_agent(&agent, credentials),
        Err(error) => Err(error),
    }
}

pub(in crate::providers) fn renew(
    credentials: Credentials,
    cancelled: &AtomicBool,
) -> Result<Session, ProviderError> {
    check_cancelled(cancelled)?;
    renew_with_agent(&agent(), credentials)
}

fn renew_with_agent(
    agent: &ureq::Agent,
    mut credentials: Credentials,
) -> Result<Session, ProviderError> {
    let Credentials::YandexMusic {
        access_token,
        refresh_token,
        access_expires_at,
    } = &mut credentials
    else {
        unreachable!()
    };
    let saved_refresh = refresh_token
        .as_deref()
        .filter(|token| valid_token(token, TOKEN_LIMIT))
        .ok_or_else(expired_connection)?;
    let updated = refresh(agent, saved_refresh)?;
    // A successful exchange may invalidate the saved token. Confirm the
    // updated account so the caller can persist it even after cancellation.
    let profile = confirm_profile(agent, &updated.access_token)?;
    *access_token = updated.access_token;
    *access_expires_at = updated.access_expires_at;
    if let Some(rotated) = updated.refresh_token {
        *refresh_token = Some(rotated);
    }
    Ok(Session::new(profile, credentials))
}

fn refresh(agent: &ureq::Agent, refresh_token: &str) -> Result<Grant, ProviderError> {
    let (status, response) = exchange(agent, "refresh_token", ("refresh_token", refresh_token))
        .map_err(|error| match error.kind {
            ErrorKind::Expired | ErrorKind::Rejected | ErrorKind::InvalidResponse => {
                unavailable_connection()
            }
            _ => error,
        })?;
    match (status, response["error"].as_str()) {
        (200, _) => grant(&response).map_err(|_| unavailable_connection()),
        (400, Some("invalid_grant" | "expired_token" | "unsupported_grant_type")) => {
            Err(expired_connection())
        }
        _ => Err(unavailable_connection()),
    }
}

fn exchange(
    agent: &ureq::Agent,
    grant_type: &str,
    grant_parameter: (&str, &str),
) -> Result<(u16, Value), ProviderError> {
    let mut response = agent
        .post(&endpoint(OAUTH, TOKEN_PATH))
        .send_form([
            ("grant_type", grant_type),
            grant_parameter,
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
        ])
        .map_err(|error| network(PROVIDER, error))?;
    let status = response.status().as_u16();
    let http_error = response_error(PROVIDER, status);
    if http_error.kind == ErrorKind::Network {
        return Err(http_error);
    }
    let body = decode_body(PROVIDER, &mut response)
        .map_err(|error| if status == 200 { error } else { http_error })?;
    Ok((status, body))
}

fn grant(response: &Value) -> Result<Grant, ProviderError> {
    let access_token = field(response, "access_token")
        .filter(|token| valid_token(token, TOKEN_LIMIT))
        .ok_or_else(|| invalid_response(PROVIDER))?;
    let refresh_token =
        field(response, "refresh_token").filter(|token| valid_token(token, TOKEN_LIMIT));
    let access_expires_at = expiration_after(response["expires_in"].as_u64());
    Ok(Grant {
        access_token,
        refresh_token,
        access_expires_at,
    })
}

pub(super) fn confirm_profile(
    agent: &ureq::Agent,
    access_token: &str,
) -> Result<Profile, ProviderError> {
    let data = get(agent, "/account/status", access_token).map_err(|error| match error.kind {
        ErrorKind::Rejected | ErrorKind::InvalidResponse => unavailable_connection(),
        ErrorKind::Expired => expired_connection(),
        _ => error,
    })?;
    let account = &data["result"]["account"];
    let id = numeric_id(&account["uid"]).ok_or_else(unavailable_connection)?;
    let name = field(account, "displayName")
        .or_else(|| field(account, "login"))
        .ok_or_else(unavailable_connection)?;
    Ok(Profile { id, name })
}

fn verification_url(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    if !matches!(url.path(), "/device" | "/device/") {
        return None;
    }
    https_url(value, "oauth.yandex.ru").or_else(|| https_url(value, "ya.ru"))
}

fn expired_connection() -> ProviderError {
    ProviderError::new(
        ErrorKind::Expired,
        "Подключение Яндекс Музыки истекло. Войдите снова по коду устройства.",
    )
}

fn unavailable_connection() -> ProviderError {
    ProviderError::new(
        ErrorKind::Unavailable,
        "Яндекс Музыка временно не подтвердила подключение. Повторите проверку.",
    )
}

#[cfg(all(test, windows))]
mod tests;
