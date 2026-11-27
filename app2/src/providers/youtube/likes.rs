use std::sync::atomic::AtomicBool;

use serde_json::json;

use crate::providers::{
    Credentials, ErrorKind, Profile, ProviderError,
    transport::{agent, check_cancelled, invalid_response},
};

use super::{Auth, PROVIDER, account, request_with_agent, signed_out};

pub(in crate::providers) fn set(
    credentials: &Credentials,
    profile: &Profile,
    id: &str,
    liked: bool,
    cancelled: &AtomicBool,
) -> Result<(), ProviderError> {
    let session = account::restore(credentials.clone(), profile, cancelled)?;
    check_cancelled(cancelled)?;
    let action = if liked {
        "like/like"
    } else {
        "like/removelike"
    };
    let response = request_with_agent(
        &agent(),
        false,
        action,
        json!({"target": {"videoId": id}}),
        Some(Auth::from_credentials(&session.credentials)),
        None,
        None,
    )?;
    if signed_out(&response) {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "Войдите в YouTube Music снова.",
        ));
    }
    if response["actions"].as_array().is_some_and(|actions| {
        actions
            .iter()
            .any(|action| action.get("showEngagementPanelEndpoint").is_some())
    }) {
        return Err(ProviderError::new(
            ErrorKind::Rejected,
            "YouTube Music требует подтверждения действия на площадке.",
        ));
    }
    if response.get("error").is_some() || !response["responseContext"].is_object() {
        return Err(invalid_response(PROVIDER));
    }
    Ok(())
}
