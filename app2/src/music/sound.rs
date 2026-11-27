use std::sync::atomic::{AtomicBool, Ordering};

use crate::{
    playlist::{Media, PlaylistKey, PlaylistTrack},
    providers::{ProviderId, catalog},
    storage::{Account, Store},
};

use super::{
    MusicPage, ObjectKind, ObjectRef, isrc, links, normalized, page_cache::PageQuery,
    recording_title, region, service::cached_links,
};

const STREAM_PROVIDERS: [ProviderId; 4] = [
    ProviderId::YandexMusic,
    ProviderId::Jellyfin,
    ProviderId::SoundCloud,
    ProviderId::Spotify,
];
const MAX_METADATA_CHECKS: usize = 4;
const DURATION_TOLERANCE_SECS: u32 = 2;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AlternateStream {
    pub provider: ProviderId,
    pub profile: String,
    pub id: String,
    pub evidence: &'static str,
}

pub(crate) fn recording(playlist: &PlaylistKey, track: &PlaylistTrack) -> Option<ObjectRef> {
    let (provider, id) = match &track.media {
        Some(Media::Provider { provider, id }) => (*provider, id.as_str()),
        None if !track.artist.trim().is_empty() || track.duration.is_some() => {
            (playlist.provider()?, track.id.as_str())
        }
        _ => return None,
    };
    let reference = ObjectRef {
        provider,
        kind: ObjectKind::Track,
        id: id.to_owned(),
    };
    reference.is_valid().then_some(reference)
}

pub(crate) fn recording_isrc(
    store: &Store,
    playlist: &PlaylistKey,
    track: &PlaylistTrack,
    cancelled: &AtomicBool,
) -> Result<Option<String>, String> {
    check_cancelled(cancelled)?;
    let Some(source) = recording(playlist, track) else {
        return Ok(None);
    };
    let profile = playlist
        .connected_source()
        .filter(|(provider, _)| *provider == source.provider)
        .and_then(|(_, profile)| profile);
    let query = PageQuery::resolve(store, &source, profile, &region())
        .map_err(|error| error.to_string())?;
    if let Some(cached) = query
        .cached()
        .map_err(|error| error.to_string())?
        .and_then(|page| isrc(page.data.isrc.as_deref()))
    {
        return Ok(Some(cached));
    }
    // YouTube Music track pages currently have no ISRC. Confirmed links are
    // checked separately, so querying the same metadata here cannot help.
    if source.provider == ProviderId::YouTubeMusic {
        return Ok(None);
    }
    let page = query
        .refresh(cancelled)
        .map_err(|error| error.to_string())?;
    check_cancelled(cancelled)?;
    Ok(isrc(page.isrc.as_deref()))
}

pub(crate) fn confirmed_streams(
    store: &Store,
    playlist: &PlaylistKey,
    track: &PlaylistTrack,
) -> Result<Vec<AlternateStream>, String> {
    let Some(source) = recording(playlist, track) else {
        return Ok(Vec::new());
    };
    let region = region();
    let known = cached_links(store, &region, &source).map_err(|error| error.to_string())?;
    let mut candidates = Vec::new();
    for link in known {
        let Some(reference) = ObjectRef::from_url(&link.url)
            .filter(|item| item.kind == ObjectKind::Track && item.provider != source.provider)
        else {
            continue;
        };
        if !STREAM_PROVIDERS.contains(&reference.provider) {
            continue;
        }
        let Some(account) = store
            .preferred_account(reference.provider)
            .map_err(|error| error.to_string())?
        else {
            continue;
        };
        let native = if let Some(public) = links::public_link(&link.url) {
            store
                .native_music_alias(&region, &public)
                .map_err(|error| error.to_string())?
                .filter(|item| {
                    item.provider == reference.provider && item.kind == ObjectKind::Track
                })
        } else {
            None
        };
        candidates.push(AlternateStream {
            provider: reference.provider,
            profile: account.profile.id,
            id: native.unwrap_or(reference).id,
            evidence: "Подтверждённая музыкальная ссылка",
        });
    }
    Ok(candidates)
}

pub(crate) fn alternate_streams(
    store: &Store,
    playlist: &PlaylistKey,
    track: &PlaylistTrack,
    wanted: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<Vec<AlternateStream>, String> {
    check_cancelled(cancelled)?;
    let (Some(source), Some(wanted)) = (recording(playlist, track), isrc(wanted)) else {
        return Ok(Vec::new());
    };
    let accounts = STREAM_PROVIDERS
        .into_iter()
        .filter(|provider| *provider != source.provider)
        .map(|provider| store.preferred_account(provider))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut candidates = Vec::new();
    let mut last_error = None;
    for account in accounts.into_iter().flatten() {
        check_cancelled(cancelled)?;
        match find_isrc(store, account, track, &wanted, cancelled) {
            Ok(Some(candidate)) => candidates.push(candidate),
            Ok(None) => {}
            Err(error) => last_error = Some(error),
        }
    }
    check_cancelled(cancelled)?;
    if candidates.is_empty()
        && let Some(error) = last_error
    {
        return Err(error);
    }
    Ok(candidates)
}

fn find_isrc(
    store: &Store,
    account: Account,
    track: &PlaylistTrack,
    wanted: &str,
    cancelled: &AtomicBool,
) -> Result<Option<AlternateStream>, String> {
    let provider = account.provider;
    let Some(credentials) = store
        .credentials(&account)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    // The text query finds candidate rows even at providers whose search API
    // has no ISRC filter. Only confirmed ISRC equality admits a candidate.
    let query = format!("{} {}", track.title.trim(), track.artist.trim());
    let rows = catalog::search(
        provider,
        &credentials,
        ObjectKind::Track,
        query.trim(),
        cancelled,
    )
    .map_err(|error| error.to_string())?;
    let found = rows
        .iter()
        .find(|page| isrc_matches(track, wanted, page))
        .map(|page| page.reference.id.clone());
    if let Some(id) = found {
        return Ok(Some(isrc_candidate(account, id)));
    }
    let title = normalized(&track.title);
    let region = region();
    for row in rows
        .into_iter()
        .filter(|page| {
            isrc(page.isrc.as_deref()).is_none()
                && (normalized(&page.title) == title || recording_title(page) == title)
        })
        .take(MAX_METADATA_CHECKS)
    {
        check_cancelled(cancelled)?;
        if let Ok(page) =
            PageQuery::for_account(store, &row.reference, Some(account.clone()), &region)
                .refresh(cancelled)
            && isrc_matches(track, wanted, &page)
        {
            return Ok(Some(isrc_candidate(account, page.reference.id.clone())));
        }
    }
    Ok(None)
}

fn isrc_matches(track: &PlaylistTrack, wanted: &str, page: &MusicPage) -> bool {
    isrc(page.isrc.as_deref()).as_deref() == Some(wanted)
        && track
            .duration
            .zip(page.duration)
            .is_none_or(|(seconds, ms)| seconds.abs_diff(ms / 1000) <= DURATION_TOLERANCE_SECS)
}

fn isrc_candidate(account: Account, id: String) -> AlternateStream {
    AlternateStream {
        provider: account.provider,
        profile: account.profile.id,
        id,
        evidence: "Совпадение ISRC",
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err("Поиск источника звука отменён.".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_recordings_can_fall_back_and_removed_occurrences_cannot() {
        let playlist = PlaylistKey::Source {
            provider: ProviderId::YouTubeMusic,
            profile: None,
            id: "PLExample".into(),
        };
        let mut track = PlaylistTrack {
            id: "dQw4w9WgXcQ".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: None,
            artwork: None,
            added: None,
            duration: Some(180),
            media: None,
        };
        let reference = recording(&playlist, &track).unwrap();
        assert_eq!(reference.provider, ProviderId::YouTubeMusic);
        assert_eq!(reference.id, track.id);
        assert!(recording(&PlaylistKey::Local(1), &track).is_none());
        let mut page = MusicPage::new(reference, "Song".into());
        page.duration = Some(180_600);
        assert!(!isrc_matches(&track, "USABC2400123", &page));
        page.isrc = Some("us-abc-24-00123".into());
        assert!(isrc_matches(&track, "USABC2400123", &page));
        page.duration = Some(200_000);
        assert!(!isrc_matches(&track, "USABC2400123", &page));
        track.title = "Трек недоступен".into();
        track.artist.clear();
        track.duration = None;
        assert!(recording(&playlist, &track).is_none());
    }
}
