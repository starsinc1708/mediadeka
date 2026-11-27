use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::{
    providers::{ErrorKind, ProviderError, ProviderId, catalog},
    storage::{Account, Store},
    telemetry::{self, CatalogAccess},
};

use super::{
    ConfirmedLink, MusicPage, ObjectKind, ObjectRef, links, now,
    page_cache::{CachedPage, PageQuery, PageReceipt},
    region, same_object,
};

#[cfg(all(test, windows))]
mod acceptance;
mod connected_matches;

const MATCH_LEASE_SECONDS: u64 = 5 * 60;
const MATCH_RETRY_SECONDS: u64 = 60 * 60;
const MATCH_EMPTY_SECONDS: u64 = 24 * 60 * 60;
const MATCH_COMPLETE: u64 = i64::MAX as u64;
const MAX_CANDIDATES_TO_VERIFY: usize = 4;

#[derive(Clone, Debug)]
pub(crate) struct ScopedPage {
    pub page: Arc<MusicPage>,
    pub profile: Option<String>,
    pub receipt: Option<PageReceipt>,
}

pub(crate) enum PageUpdate {
    Data(ScopedPage),
    Links(Vec<ConfirmedLink>),
    Error {
        message: String,
        hide: bool,
        needs_sign_in: bool,
    },
}

struct LoadFailure {
    message: String,
    hide: bool,
    needs_sign_in: bool,
}

impl LoadFailure {
    fn access(message: impl Into<String>, hide: bool) -> Self {
        Self {
            message: message.into(),
            hide,
            needs_sign_in: true,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct MatchSnapshot {
    links: Vec<ConfirmedLink>,
}

pub(super) fn confirmed_music_pairs(
    store: &Store,
    region: &str,
    kind: ObjectKind,
) -> Result<Vec<(ObjectRef, ObjectRef)>, crate::storage::StoreError> {
    let matches = store.music_matches::<MatchSnapshot>(region, kind)?;
    let aliases = store.native_music_aliases(region, kind)?;
    // SoundCloud publishes albums under /sets/. Use an album alias only if
    // the saved lookup confirmed both its identity and its catalog type.
    let soundcloud_albums = aliases
        .iter()
        .filter(|(alias, native)| {
            alias.kind == ObjectKind::Album
                && alias.provider == ProviderId::SoundCloud
                && native.provider == ProviderId::SoundCloud
        })
        .map(|(alias, _)| alias.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut pairs = Vec::new();
    for (source, snapshot) in matches {
        for link in snapshot.links {
            let Some(mut target) = ObjectRef::from_url(&link.url) else {
                continue;
            };
            if target.kind != kind {
                if kind != ObjectKind::Album
                    || target.kind != ObjectKind::Playlist
                    || target.provider != ProviderId::SoundCloud
                    || !soundcloud_albums.contains(target.id.as_str())
                {
                    continue;
                }
                target.kind = ObjectKind::Album;
            }
            if target != source {
                pairs.push((source.clone(), target));
            }
        }
    }
    pairs.extend(aliases);
    Ok(pairs)
}

pub(super) fn cached_links(
    store: &Store,
    region: &str,
    object: &ObjectRef,
) -> Result<Vec<ConfirmedLink>, crate::storage::StoreError> {
    Ok(store
        .music_match::<MatchSnapshot>(region, object)?
        .map_or_else(Vec::new, |snapshot| snapshot.links))
}

pub(crate) fn load_page(
    object: &ObjectRef,
    profile: Option<&str>,
    cancelled: &AtomicBool,
    emit: impl Fn(PageUpdate),
) {
    if let Err(LoadFailure {
        message,
        hide,
        needs_sign_in,
    }) = load(object, profile, cancelled, &emit)
        && !cancelled.load(Ordering::Relaxed)
    {
        emit(PageUpdate::Error {
            message,
            hide,
            needs_sign_in,
        });
    }
}

pub(crate) fn prefetch_page(
    object: &ObjectRef,
    profile: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<Option<ScopedPage>, ProviderError> {
    if matches!(object.kind, ObjectKind::Playlist | ObjectKind::Station) {
        return Ok(None);
    }
    let store = Store::open_default().map_err(|error| ProviderError {
        kind: ErrorKind::InvalidResponse,
        message: error.to_string(),
    })?;
    let query = PageQuery::resolve(&store, object, profile, &region())?;
    let page = query.refresh_cached(cancelled)?;
    Ok(Some(scoped_cache(page, &query)))
}

fn load(
    object: &ObjectRef,
    profile: Option<&str>,
    cancelled: &AtomicBool,
    emit: &impl Fn(PageUpdate),
) -> Result<(), LoadFailure> {
    let store = Store::open_default().map_err(display_error)?;
    let region = region();
    let query = PageQuery::resolve(&store, object, profile, &region).map_err(|error| {
        if error.needs_sign_in() {
            LoadFailure::access(error.message, true)
        } else {
            display_error(error)
        }
    })?;
    let cached = match query.cached() {
        Ok(page) => page,
        Err(error) => {
            emit(PageUpdate::Error {
                message: error.to_string(),
                hide: false,
                needs_sign_in: false,
            });
            None
        }
    };
    let fresh = cached.as_ref().is_some_and(|page| page.fresh(object.kind));
    telemetry::catalog_open(match (&cached, fresh) {
        (_, true) => CatalogAccess::Fresh,
        (Some(_), false) => CatalogAccess::Stale,
        (None, false) => CatalogAccess::Miss,
    });
    let mut confirmed = cached_links(&store, &region, object).map_err(display_error)?;
    if let Some(page) = &cached {
        if let Some(link) = source_link(&page.data) {
            insert_link(&mut confirmed, link);
        }
        emit(PageUpdate::Data(scoped_cache(page.clone(), &query)));
    } else if let Some(page) = cached_matching_page(&store, &region, object, &query, &confirmed)? {
        emit(PageUpdate::Data(page));
    }
    if !confirmed.is_empty() {
        emit(PageUpdate::Links(confirmed.clone()));
    }

    let page = if let Some(page) = cached.filter(|_| fresh) {
        scoped_cache(page, &query)
    } else {
        let page = query.refresh_cached(cancelled).map_err(|error| {
            if error.needs_sign_in() {
                let message = if query.account().is_none() {
                    format!(
                        "Для этой страницы нужно подключение {}.",
                        object.provider.display_name()
                    )
                } else {
                    error.message
                };
                LoadFailure::access(message, error.kind == ErrorKind::Revoked)
            } else {
                display_error(error)
            }
        })?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        let page = scoped_cache(page, &query);
        emit(PageUpdate::Data(page.clone()));
        page
    };

    let source = source_link(&page.page);
    let has_source = source.is_some();
    if let Some(source) = source
        && !confirmed
            .iter()
            .any(|link| link.platform == source.platform)
    {
        insert_link(&mut confirmed, source);
        emit(PageUpdate::Links(confirmed.clone()));
    }
    if cancelled.load(Ordering::Relaxed)
        || page.page.title.is_empty()
        || object.kind == ObjectKind::Collection
    {
        return Ok(());
    }
    let started = now();
    let lease = started + MATCH_LEASE_SECONDS;
    let missing_spotify_release = object.provider != ProviderId::Spotify
        && connected_matches::track_release(&page.page).is_some()
        && confirmed
            .iter()
            .all(|link| link.platform != ProviderId::Spotify.as_str())
        && store
            .preferred_account(ProviderId::Spotify)
            .map_err(display_error)?
            .is_some();
    let claimed = store
        .claim_music_match(&region, object, started, lease)
        .map_err(display_error)?;
    if !(claimed
        || missing_spotify_release
            && store
                .reopen_completed_music_match(&region, object, ProviderId::Spotify, lease)
                .map_err(display_error)?)
    {
        return Ok(());
    }

    let mut aliases = vec![object.clone()];
    if page.page.reference != *object
        && page.page.reference.kind == object.kind
        && page.page.reference.provider == object.provider
    {
        aliases.push(page.page.reference.clone());
    }
    let (links, catalog_failed) = connected_matches::find(
        &store,
        &region,
        &page.page,
        query.account(),
        &confirmed,
        cancelled,
    )
    .map_err(display_error)?;
    for (alias, link) in links {
        aliases.push(alias);
        insert_link(&mut confirmed, link);
    }
    emit(PageUpdate::Links(confirmed.clone()));
    let resolution = links::resolve(&page.page, &confirmed, &region, cancelled);
    for link in resolution.links {
        insert_link(&mut confirmed, link);
    }
    for alias in confirmed
        .iter()
        .filter_map(|link| ObjectRef::from_url(&link.url))
        .filter(|alias| alias.kind == object.kind)
    {
        if !aliases.contains(&alias) {
            aliases.push(alias);
        }
    }
    let spotify_pending = missing_spotify_release
        && confirmed
            .iter()
            .all(|link| link.platform != ProviderId::Spotify.as_str());
    let retry_at = if confirmed.len() > usize::from(has_source) && !spotify_pending {
        MATCH_COMPLETE
    } else if let Some(retry) = resolution.retry_after {
        now().saturating_add(retry.as_secs().max(MATCH_RETRY_SECONDS))
    } else if cancelled.load(Ordering::Relaxed) || catalog_failed {
        now() + MATCH_RETRY_SECONDS
    } else {
        now() + MATCH_EMPTY_SECONDS
    };
    store
        .save_music_match(
            &region,
            &aliases,
            lease,
            retry_at,
            &MatchSnapshot {
                links: confirmed.clone(),
            },
        )
        .map_err(display_error)?;
    if has_source && confirmed.len() > 1 {
        store
            .remember_music_aliases(&region, object, &confirmed)
            .map_err(display_error)?;
    }
    emit(PageUpdate::Links(confirmed));
    Ok(())
}

fn cached_matching_page(
    store: &Store,
    region: &str,
    object: &ObjectRef,
    requested: &PageQuery<'_>,
    confirmed: &[ConfirmedLink],
) -> Result<Option<ScopedPage>, LoadFailure> {
    let canonical = object
        .url()
        .as_deref()
        .and_then(links::public_link)
        .map(|link| store.native_music_alias(region, &link))
        .transpose()
        .map_err(display_error)?
        .flatten();
    let alternatives = ProviderId::ALL.into_iter().filter_map(|provider| {
        let mut candidates = confirmed
            .iter()
            .filter(|link| link.platform == provider.as_str())
            .filter_map(|link| ObjectRef::from_url(&link.url))
            .filter(|candidate| candidate.kind == object.kind && candidate.provider == provider);
        let candidate = candidates.next()?;
        (Some(&candidate) != canonical.as_ref() && candidates.all(|other| other == candidate))
            .then_some(candidate)
    });
    for alternate in canonical.iter().cloned().chain(alternatives) {
        if &alternate == object || alternate.kind != object.kind || !alternate.is_valid() {
            continue;
        }
        // A same-provider alias belongs to the requested profile. A cross-provider
        // match can only read that provider's active account or public snapshot.
        let same_provider = alternate.provider == object.provider;
        let account = if same_provider {
            requested.account().cloned()
        } else {
            store
                .preferred_account(alternate.provider)
                .map_err(display_error)?
        };
        let public = !same_provider && account.is_some();
        for account in std::iter::once(account).chain(public.then_some(None)) {
            let query = PageQuery::for_account(store, &alternate, account, region);
            if let Some(page) = query.cached().map_err(display_error)?
                && page.data.reference.provider == alternate.provider
                && page.data.reference.kind == alternate.kind
            {
                // Attribute the cached page to its actual provider/profile scope and
                // leave the requested route's prefetch receipt empty.
                return Ok(Some(scoped(page.data, query.account())));
            }
        }
    }
    Ok(None)
}

pub(super) fn scoped(page: impl Into<Arc<MusicPage>>, account: Option<&Account>) -> ScopedPage {
    ScopedPage {
        page: page.into(),
        profile: account.map(|account| account.profile.id.clone()),
        receipt: None,
    }
}

fn scoped_cache(page: CachedPage, query: &PageQuery<'_>) -> ScopedPage {
    let receipt = query.receipt(&page);
    ScopedPage {
        receipt: Some(receipt),
        ..scoped(page.data, query.account())
    }
}

fn source_link(page: &MusicPage) -> Option<ConfirmedLink> {
    let link = links::public_link(page.url.as_deref()?)?;
    let verified_soundcloud_set = page.reference.provider == ProviderId::SoundCloud
        && page.reference.kind == ObjectKind::Album
        && link.kind == ObjectKind::Playlist;
    ((link.kind == page.reference.kind || verified_soundcloud_set)
        && link.platform == page.reference.provider.as_str())
    .then_some(ConfirmedLink {
        id: link.id,
        platform: link.platform,
        url: link.url,
        evidence: "Карточка каталога".into(),
        checked_at: now(),
    })
}

fn insert_link(links: &mut Vec<ConfirmedLink>, link: ConfirmedLink) {
    if !links.iter().any(|known| known.platform == link.platform) {
        links.push(link);
    }
}

pub(crate) enum PublicInput {
    Address(ObjectRef, String),
    Candidates(Vec<ScopedPage>),
    Unresolved {
        provider: ProviderId,
        url: String,
        error: ProviderError,
    },
}

pub(crate) fn resolve_public_input(
    value: &str,
    cancelled: &AtomicBool,
    emit: impl Fn(PublicInput) + Sync,
) -> Result<(), String> {
    use links::short::ShortLinkError;

    if let Some(link) = links::jellyfin::DetailsLink::parse(value) {
        let store = Store::open_default().map_err(|error| error.to_string())?;
        let (object, url) = link.resolve(&store, cancelled)?;
        if !cancelled.load(Ordering::Relaxed) {
            emit(PublicInput::Address(object, url));
        }
        return Ok(());
    }

    let link = links::public_link(value)
        .map_or_else(|| links::short::resolve(value, cancelled), Ok)
        .map_err(|error| {
            match error {
                ShortLinkError::NotShort => {
                    "Введите прямую ссылку на музыкальный объект поддерживаемой площадки."
                }
                ShortLinkError::ForeignHost => "Короткая ссылка ведёт на неподдерживаемый домен.",
                ShortLinkError::UnsafeAddress => "Короткая ссылка ведёт на небезопасный адрес.",
                ShortLinkError::TooManyRedirects => {
                    "Слишком много переадресаций в короткой ссылке."
                }
                ShortLinkError::Network => {
                    "Не удалось загрузить короткую ссылку. Попробуйте позже."
                }
                ShortLinkError::NoObject => {
                    "Короткая ссылка не ведёт на конкретный музыкальный объект."
                }
                ShortLinkError::Cancelled => "Открытие короткой ссылки отменено.",
            }
            .to_owned()
        })?;
    let store = Store::open_default().map_err(|error| error.to_string())?;
    if let Some(object) = ObjectRef::from_url(&link.url) {
        let provider = object.provider;
        let result = if is_soundcloud_set(&object) {
            resolve_soundcloud_public_set(&store, &object, cancelled)
        } else {
            Ok(object)
        };
        if !cancelled.load(Ordering::Relaxed) {
            emit(match result {
                Ok(object) => PublicInput::Address(object, link.url),
                Err(error) => PublicInput::Unresolved {
                    provider,
                    url: link.url,
                    error,
                },
            });
        }
        return Ok(());
    }
    let region = region();
    if let Some(object) = store
        .native_music_alias(&region, &link)
        .map_err(|error| error.to_string())?
    {
        emit(PublicInput::Address(object, link.url));
        return Ok(());
    }
    let source: links::PublicMetadata = links::public_metadata(&link, &region, cancelled)
        .ok_or_else(|| "Площадка не вернула карточку по этой ссылке.".to_owned())?;
    let query = format!("{} {}", source.title, source.artists.join(" "));
    let source = Arc::new(source);
    let found = AtomicBool::new(false);
    super::search_cache::search_catalogs(
        link.kind,
        query.trim(),
        super::search_cache::Dispatch::ResolveLink,
        cancelled,
        |result| {
            if let Ok(batch) = result
                && batch.complete
            {
                let matching = batch
                    .pages
                    .iter()
                    .filter(|candidate| {
                        super::normalized(&candidate.page.title) == super::normalized(&source.title)
                    })
                    .take(MAX_CANDIDATES_TO_VERIFY)
                    .filter_map(|candidate| {
                        // The catalogue ID is irrelevant to comparing a public recording's metadata.
                        let external = (*source)
                            .clone()
                            .into_page(candidate.page.reference.clone());
                        let profile = candidate.profile.as_deref()?;
                        let account = store
                            .account(candidate.page.reference.provider, profile)
                            .ok()??;
                        let detail = PageQuery::for_account(
                            &store,
                            &candidate.page.reference,
                            Some(account.clone()),
                            &region,
                        )
                        .refresh(cancelled)
                        .ok()?;
                        same_object(&external, &detail)?;
                        Some(scoped(detail, Some(&account)))
                    })
                    .collect::<Vec<_>>();
                if !matching.is_empty() {
                    found.store(true, Ordering::Relaxed);
                    emit(PublicInput::Candidates(matching));
                }
            }
        },
    )?;
    if !found.load(Ordering::Relaxed) {
        return Err("Не удалось подтвердить эту версию в подключённых каталогах.".into());
    }
    Ok(())
}

pub(crate) fn is_soundcloud_set(reference: &ObjectRef) -> bool {
    reference.provider == ProviderId::SoundCloud
        && reference.kind == ObjectKind::Playlist
        && reference.id.contains('/')
}

fn resolve_soundcloud_public_set(
    store: &Store,
    object: &ObjectRef,
    cancelled: &AtomicBool,
) -> Result<ObjectRef, ProviderError> {
    let storage_error = |error: crate::storage::StoreError| ProviderError {
        kind: ErrorKind::InvalidResponse,
        message: error.to_string(),
    };
    let access_error = |kind, message: &str| ProviderError {
        kind,
        message: message.into(),
    };
    let account = store
        .preferred_account(object.provider)
        .map_err(storage_error)?
        .ok_or_else(|| {
            access_error(
                ErrorKind::Expired,
                "Для этой ссылки нужно подключение SoundCloud.",
            )
        })?;
    if store.account_revoked(&account).map_err(storage_error)? {
        return Err(access_error(
            ErrorKind::Revoked,
            "Подтвердите вход в SoundCloud заново.",
        ));
    }
    let credentials = store
        .credentials(&account)
        .map_err(storage_error)?
        .ok_or_else(|| access_error(ErrorKind::Expired, "Подтвердите вход в SoundCloud заново."))?;
    catalog::resolve_soundcloud_set(object, &credentials, cancelled)
}

fn display_error(error: impl std::fmt::Display) -> LoadFailure {
    LoadFailure {
        message: error.to_string(),
        hide: false,
        needs_sign_in: false,
    }
}
