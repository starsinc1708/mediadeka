use std::{
    collections::HashSet,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::{
    Auth, Credentials, ErrorKind, ProviderError, agent, check_cancelled, field, named, proof,
    request_with_agent, signed_out, text,
};
use crate::providers::{Profile, Session};

const PROFILE_LIMIT: usize = 8;
const PROBE_BUDGET: Duration = Duration::from_secs(10);
const PROFILE_UNAVAILABLE: &str =
    "YouTube Music пока не подтвердил сохранённый профиль. Попробуйте ещё раз.";
const COOKIE_EXPIRED: &str =
    "YouTube Music подтвердил выход из аккаунта. Обновите подключение из браузера.";

pub struct YouTubeChoices {
    cookies: String,
    profiles: Vec<Profile>,
    locations: Vec<Location>,
}

impl YouTubeChoices {
    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    pub(in crate::providers) fn choose(
        self,
        id: &str,
        cancelled: &AtomicBool,
    ) -> Result<Session, ProviderError> {
        let at = self
            .profiles
            .iter()
            .position(|profile| profile.id == id)
            .ok_or_else(|| {
                ProviderError::new(
                    ErrorKind::InvalidInput,
                    "Этот профиль YouTube Music недоступен.",
                )
            })?;
        let location = &self.locations[at];
        let mut profile = Probe::new(cancelled).verify(&self.cookies, location)?;
        profile.id = id.to_owned();
        check_cancelled(cancelled)?;
        Ok(Session::new(profile, location.credentials(self.cookies)))
    }
}

#[derive(Clone)]
struct Identity {
    profile: Profile,
    handle: Option<String>,
    page_id: Option<String>,
    selected: bool,
}

impl Identity {
    fn read(node: &Value) -> Option<Self> {
        let handle = text(&node["channelHandle"])
            .or_else(|| text(&node["accountByline"]).filter(|value| value.starts_with('@')));
        let email = text(&node["email"])
            .or_else(|| text(&node["accountByline"]).filter(|value| value.contains('@')));
        let name = text(&node["accountName"])
            .or_else(|| handle.clone())
            .or_else(|| email.clone())?;
        let mut tokens = Vec::new();
        named(node, "pageIdToken", &mut tokens);
        let page_id = tokens
            .into_iter()
            .find_map(|token| field(token, "pageId"))
            .filter(|id| super::feed::safe_id(id));
        let id = page_id.clone().or_else(|| handle.clone()).or(email)?;
        Some(Self {
            profile: Profile { id, name },
            handle,
            page_id,
            selected: node["isSelected"] == true,
        })
    }

    fn matches_id(&self, id: &str) -> bool {
        self.profile.id == id || self.handle.as_deref() == Some(id)
    }

    fn matches_saved(&self, id: &str, page_id: Option<&str>) -> bool {
        self.matches_id(id)
            && page_id
                .zip(self.page_id.as_deref())
                .is_none_or(|(saved, current)| saved == current)
    }

    fn matches_identity(&self, other: &Self) -> bool {
        if let (Some(left), Some(right)) = (&self.page_id, &other.page_id) {
            return left == right;
        }
        self.matches_id(&other.profile.id) || other.matches_id(&self.profile.id)
    }

    fn verify(&self, menu: Result<Self, ProviderError>) -> Result<Profile, ProviderError> {
        match menu {
            Ok(active) if active.matches_identity(self) => Ok(active.profile),
            // The signed-in switcher can confirm its explicitly selected account
            // when the menu does not contain an active profile.
            Err(error) if error.kind == ErrorKind::Unavailable && self.selected => {
                Ok(self.profile.clone())
            }
            Err(error) => Err(error),
            Ok(_) => Err(profile_unavailable()),
        }
    }
}

struct Location {
    index: usize,
    identity: Identity,
}

impl Location {
    fn credentials(&self, cookies: String) -> Credentials {
        Credentials::YouTubeMusic {
            cookies,
            index: self.index,
            page_id: self.identity.page_id.clone(),
        }
    }
}

pub fn youtube_profiles(
    cookies: String,
    cancelled: &AtomicBool,
) -> Result<YouTubeChoices, ProviderError> {
    let cookies = normalize_cookies(&cookies)?;
    let probe = Probe::new(cancelled);
    let mut profiles = Vec::new();
    let mut locations = Vec::new();
    let mut seen = HashSet::new();
    for index in 0..PROFILE_LIMIT {
        check_cancelled(cancelled)?;
        let auth = Auth {
            cookies: &cookies,
            index,
            page_id: None,
        };
        let candidates = match probe.accounts(auth) {
            Ok(accounts) if !accounts.is_empty() => accounts,
            list => match probe.menu(auth) {
                Ok(identity) => vec![identity],
                Err(_) if !profiles.is_empty() => break,
                Err(error) => return Err(prefer_confirmed_rejection(error, list.as_ref().err())),
            },
        };
        let previous = profiles.len();
        for identity in candidates {
            if seen.insert(identity.profile.id.clone()) {
                profiles.push(identity.profile.clone());
                locations.push(Location { index, identity });
            }
        }
        if previous == profiles.len() {
            break;
        }
    }
    check_cancelled(cancelled)?;
    Ok(YouTubeChoices {
        cookies,
        profiles,
        locations,
    })
}

pub(in crate::providers) fn sign_in(
    cookies: String,
    cancelled: &AtomicBool,
) -> Result<Session, ProviderError> {
    let cookies = normalize_cookies(&cookies)?;
    let index = 0;
    let auth = Auth {
        cookies: &cookies,
        index,
        page_id: None,
    };
    let probe = Probe::new(cancelled);
    let identity = match probe.menu(auth) {
        Ok(identity) => identity,
        Err(menu) => match probe.accounts(auth) {
            Ok(accounts) => selected_account(accounts).ok_or(menu)?,
            Err(list) => return Err(prefer_confirmed_rejection(menu, Some(&list))),
        },
    };
    check_cancelled(cancelled)?;
    let profile = identity.profile.clone();
    Ok(Session::new(
        profile,
        Location { index, identity }.credentials(cookies),
    ))
}

pub(in crate::providers) fn restore(
    credentials: Credentials,
    previous: &Profile,
    cancelled: &AtomicBool,
) -> Result<Session, ProviderError> {
    let auth = Auth::from_credentials(&credentials);
    check_cancelled(cancelled)?;
    if proof(auth.cookies).is_none() {
        return Err(cookie_expired());
    }
    let probe = Probe::new(cancelled);
    let initial = probe.menu(auth);
    if let Ok(identity) = &initial
        && identity.matches_saved(&previous.id, auth.page_id)
    {
        let profile = Profile {
            id: previous.id.clone(),
            name: identity.profile.name.clone(),
        };
        return Ok(Session::new(profile, credentials));
    }
    if let Err(error) = &initial
        && matches!(error.kind, ErrorKind::Network | ErrorKind::Cancelled)
    {
        return Err(error.clone());
    }

    // The account switcher can move the same channel to another authuser or page selector.
    // Keep the persisted ID and use a new selector only after proving the channel behind it.
    let mut signed_in = initial.is_ok();
    let mut default_signed_out = false;
    let mut stored_signed_out = false;
    for index in
        std::iter::once(auth.index).chain((0..PROFILE_LIMIT).filter(|index| *index != auth.index))
    {
        if probe.expired() {
            break;
        }
        let candidates = match probe.accounts(Auth {
            index,
            page_id: None,
            ..auth
        }) {
            Ok(candidates) => candidates,
            Err(error) if error.kind == ErrorKind::Expired => {
                default_signed_out |= index == 0;
                stored_signed_out |= index == auth.index;
                continue;
            }
            Err(error) if matches!(error.kind, ErrorKind::Network | ErrorKind::Cancelled) => {
                return Err(error);
            }
            Err(_) => continue,
        };
        signed_in |= !candidates.is_empty();
        for identity in candidates
            .into_iter()
            .filter(|identity| identity.matches_saved(&previous.id, auth.page_id))
        {
            let selected = Location { index, identity };
            if let Ok(mut profile) = probe.verify(auth.cookies, &selected) {
                check_cancelled(cancelled)?;
                profile.id.clone_from(&previous.id);
                return Ok(Session::new(
                    profile,
                    selected.credentials(auth.cookies.to_owned()),
                ));
            }
            check_cancelled(cancelled)?;
        }
    }
    check_cancelled(cancelled)?;
    if !signed_in && default_signed_out && stored_signed_out {
        Err(cookie_expired())
    } else {
        Err(profile_unavailable())
    }
}

fn selected_account(accounts: Vec<Identity>) -> Option<Identity> {
    let mut selected = accounts.into_iter().filter(|account| account.selected);
    let current = selected.next()?;
    selected.next().is_none().then_some(current)
}

struct Probe<'a> {
    agent: ureq::Agent,
    cancelled: &'a AtomicBool,
    deadline: Instant,
}

impl<'a> Probe<'a> {
    fn new(cancelled: &'a AtomicBool) -> Self {
        Self {
            agent: agent(),
            cancelled,
            deadline: Instant::now() + PROBE_BUDGET,
        }
    }

    fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    fn request(
        &self,
        web: bool,
        endpoint: &str,
        payload: Value,
        auth: Auth<'_>,
    ) -> Result<Value, ProviderError> {
        check_cancelled(self.cancelled)?;
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(profile_unavailable());
        }
        let response = request_with_agent(
            &self.agent,
            web,
            endpoint,
            payload,
            Some(auth),
            None,
            Some(remaining),
        )?;
        check_cancelled(self.cancelled)?;
        if signed_out(&response) {
            return Err(cookie_expired());
        }
        Ok(response)
    }

    fn menu(&self, auth: Auth<'_>) -> Result<Identity, ProviderError> {
        let response = self.request(false, "account/account_menu", json!({}), auth)?;
        let mut accounts = Vec::new();
        named(&response, "activeAccountHeaderRenderer", &mut accounts);
        accounts
            .into_iter()
            .find_map(Identity::read)
            .ok_or_else(profile_unavailable)
    }

    fn accounts(&self, auth: Auth<'_>) -> Result<Vec<Identity>, ProviderError> {
        let response = self.request(
            true,
            "account/accounts_list",
            json!({"requestType": "ACCOUNTS_LIST_REQUEST_TYPE_ACCOUNT_SWITCHER"}),
            auth,
        )?;
        let mut accounts = Vec::new();
        named(&response, "accountItem", &mut accounts);
        Ok(accounts
            .into_iter()
            .filter(|node| node["isDisabled"] != true)
            .filter_map(Identity::read)
            .collect())
    }

    fn verify(&self, cookies: &str, location: &Location) -> Result<Profile, ProviderError> {
        let selected = &location.identity;
        let auth = Auth {
            cookies,
            index: location.index,
            page_id: selected.page_id.as_deref(),
        };
        selected.verify(self.menu(auth))
    }
}

fn profile_unavailable() -> ProviderError {
    ProviderError::new(ErrorKind::Unavailable, PROFILE_UNAVAILABLE)
}

fn cookie_expired() -> ProviderError {
    ProviderError::new(ErrorKind::Expired, COOKIE_EXPIRED)
}

fn prefer_confirmed_rejection(menu: ProviderError, list: Option<&ProviderError>) -> ProviderError {
    match list {
        Some(error) if error.kind == ErrorKind::Expired => error.clone(),
        _ => menu,
    }
}

fn normalize_cookies(input: &str) -> Result<String, ProviderError> {
    let input = input.trim();
    let input = input
        .strip_prefix("Cookie:")
        .or_else(|| input.strip_prefix("cookie:"))
        .unwrap_or(input);
    let pairs: Vec<_> = input
        .split(';')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .collect();
    if input.len() > 16 * 1024
        || pairs.iter().any(|pair| {
            !pair.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) || !pair.contains('=')
        })
    {
        return Err(ProviderError::new(
            ErrorKind::InvalidInput,
            "Введите значение заголовка Cookie из YouTube Music.",
        ));
    }
    let cookies = pairs.join("; ");
    if proof(&cookies).is_none() {
        return Err(ProviderError::new(
            ErrorKind::InvalidInput,
            "В заголовке Cookie отсутствует подтверждение входа в YouTube Music.",
        ));
    }
    Ok(cookies)
}

#[cfg(test)]
mod tests;
