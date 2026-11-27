use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    providers::{ErrorKind, catalog},
    storage::{Account, Store, StoreError},
};

use super::{
    ConfirmedLink, MAX_CANDIDATES_TO_VERIFY, MusicPage, ObjectKind, ObjectRef, PageQuery,
    same_object, source_link,
};
use crate::music::normalized;

pub(super) fn track_release(page: &MusicPage) -> Option<&ObjectRef> {
    page.album_ref.as_ref().filter(|release| {
        page.reference.kind == ObjectKind::Track
            && release.kind == ObjectKind::Album
            && release.provider == page.reference.provider
            && release.is_valid()
    })
}

pub(super) fn find(
    store: &Store,
    region: &str,
    source: &MusicPage,
    source_account: Option<&Account>,
    known: &[ConfirmedLink],
    cancelled: &AtomicBool,
) -> Result<(Vec<(ObjectRef, ConfirmedLink)>, bool), StoreError> {
    let query = format!("{} {}", source.title, source.artists.join(" "));
    let mut links = Vec::new();
    let mut failed = false;
    let mut source_album = None;
    for summary in store.accounts()? {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        let provider = summary.provider;
        if provider == source.reference.provider
            || known.iter().any(|link| link.platform == provider.as_str())
        {
            continue;
        }
        let Some(account) = store.account(provider, &summary.profile.id)? else {
            continue;
        };
        let Some(credentials) = store.credentials(&account)? else {
            continue;
        };
        let candidates = match catalog::search(
            provider,
            &credentials,
            source.reference.kind,
            query.trim(),
            cancelled,
        ) {
            Ok(candidates) => candidates,
            Err(error) => {
                failed |= error.kind == ErrorKind::Network;
                continue;
            }
        };
        let load = |reference: &ObjectRef| {
            PageQuery::for_account(store, reference, Some(account.clone()), region)
                .refresh(cancelled)
        };
        let verified = verify(source, candidates, |reference| load(reference).ok());
        let selected = select(source, verified, |origin, candidate| {
            let source_album = source_album.get_or_insert_with(|| {
                PageQuery::for_account(store, origin, source_account.cloned(), region)
                    .refresh(cancelled)
                    .ok()
            });
            let Some(source_album) = source_album else {
                return false;
            };
            match load(candidate) {
                Ok(candidate) => same_object(source_album, &candidate).is_some(),
                Err(error) => {
                    failed |= error.kind == ErrorKind::Network;
                    false
                }
            }
        });
        if let Some((page, link)) = selected {
            links.push((page.reference.clone(), link));
        }
    }
    Ok((links, failed))
}

type Verified = (Arc<MusicPage>, ConfirmedLink);

fn verify(
    source: &MusicPage,
    candidates: Vec<MusicPage>,
    mut load: impl FnMut(&ObjectRef) -> Option<Arc<MusicPage>>,
) -> Vec<Verified> {
    let title = normalized(&source.title);
    let album = release_name(source);
    let mut candidates = candidates
        .into_iter()
        .filter(|candidate| normalized(&candidate.title) == title)
        .collect::<Vec<_>>();
    if let Some(album) = &album {
        candidates.sort_by_cached_key(|candidate| release_name(candidate).as_ref() != Some(album));
    }
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.reference.clone()))
        .take(MAX_CANDIDATES_TO_VERIFY)
        .filter_map(|candidate| {
            let needs_release = track_release(source).is_some()
                && album.is_some()
                && release_name(&candidate) == album
                && track_release(&candidate).is_none();
            let page = if same_object(source, &candidate).is_some()
                && source
                    .isrc
                    .as_ref()
                    .is_none_or(|_| candidate.isrc.is_some())
                && !needs_release
            {
                Arc::new(candidate)
            } else {
                load(&candidate.reference)?
            };
            let evidence = same_object(source, &page)?;
            let mut link = source_link(&page)?;
            link.evidence = evidence.into();
            Some((page, link))
        })
        .collect()
}

fn select(
    source: &MusicPage,
    mut verified: Vec<Verified>,
    mut same_release: impl FnMut(&ObjectRef, &ObjectRef) -> bool,
) -> Option<Verified> {
    if verified.len() <= 1 {
        return verified.pop();
    }
    let name = release_name(source)?;
    let origin = track_release(source);
    let mut preferred = verified.into_iter().filter(|(page, _)| {
        release_name(page).as_ref() == Some(&name)
            && origin.is_none_or(|origin| {
                track_release(page).is_some_and(|target| same_release(origin, target))
            })
    });
    let first = preferred.next()?;
    preferred.next().is_none().then_some(first)
}

fn release_name(page: &MusicPage) -> Option<String> {
    if page.reference.kind != ObjectKind::Track {
        return None;
    }
    page.album
        .as_deref()
        .map(normalized)
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderId;

    const EP: &str = "2ed3BmK8XmEIiNF0836KVR";
    const EP_ALBUM: &str = "1zZER8YoHrHC7GZGNZZV7Z";
    const AVATAR_SINGLE: &str = "0tBvCxtBvGvwoTSZYSkXwe";

    fn track(url: &str, album: &str, album_url: &str) -> MusicPage {
        let mut page = MusicPage::new(ObjectRef::from_url(url).unwrap(), "Odyssee".into());
        page.artists = vec!["FIBEL".into()];
        page.duration = Some(189_695);
        page.album = Some(album.into());
        page.album_ref = ObjectRef::from_url(album_url);
        page
    }

    #[test]
    fn prefers_the_confirmed_edition_over_a_same_named_single() {
        let source = track(
            "https://music.yandex.ru/album/16293786/track/81439810",
            "Avatar",
            "https://music.yandex.ru/album/16293786",
        );
        let candidates = vec![
            track(
                "https://open.spotify.com/track/6ACv9l2Q7lZ9UAD92Ii8oV",
                "Odyssee",
                "https://open.spotify.com/album/4XZfgMJ7ICav1wFMoLowv7",
            ),
            track(
                "https://open.spotify.com/track/49v2rO9ixtUzluEActqiT8",
                "Avatar",
                &format!("https://open.spotify.com/album/{AVATAR_SINGLE}"),
            ),
            track(
                &format!("https://open.spotify.com/track/{EP}"),
                "Avatar",
                &format!("https://open.spotify.com/album/{EP_ALBUM}"),
            ),
        ];
        let verified = verify(&source, candidates, |_| {
            panic!("search rows have identity metadata")
        });
        assert_eq!(verified.len(), 3);
        assert!(select(&source, verified.clone(), |_, _| false).is_none());
        assert!(select(&source, verified.clone(), |_, _| true).is_none());
        let (page, link) = select(&source, verified, |origin, release| {
            assert_eq!(origin.id, "16293786");
            release.id == EP_ALBUM
        })
        .unwrap();
        assert_eq!(page.reference.id, EP);
        assert_eq!(link.platform, ProviderId::Spotify.as_str());
        assert_eq!(link.id, EP);
    }
}
