use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::{providers::ErrorKind, storage::Store};

use super::{MusicPage, ObjectKind, ObjectRef, normalized, page_cache::PageQuery, region};

pub(crate) fn read_related_album(
    album: &ObjectRef,
    profile: Option<&str>,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(Option<Arc<MusicPage>>),
) {
    if album.kind != ObjectKind::Album || !album.is_valid() || cancelled.load(Ordering::Relaxed) {
        return;
    }
    let Ok(store) = Store::open_default() else {
        return;
    };
    let region = region();
    let query = match profile {
        Some(profile) => match PageQuery::resolve(&store, album, Some(profile), &region) {
            Ok(query) => query,
            Err(_) => return,
        },
        None => PageQuery::for_account(&store, album, None, &region),
    };
    if let Ok(Some(page)) = query.cached() {
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        if page.data.reference == *album {
            emit(Some(page.data.clone()));
        }
        if page.fresh(ObjectKind::Album) {
            return;
        }
    }
    let result = query.refresh(cancelled);
    if cancelled.load(Ordering::Relaxed) {
        return;
    }
    match result {
        Ok(page) if page.reference == *album => emit(Some(page)),
        Err(error) if error.kind == ErrorKind::Revoked => emit(None),
        _ => {}
    }
}

/// Recover a catalogue relation from the exact recording behind a legacy row.
pub(crate) fn resolve_track_relation(
    origin: &ObjectRef,
    profile: Option<&str>,
    kind: ObjectKind,
    title: &str,
    cancelled: &AtomicBool,
) -> Option<ObjectRef> {
    if origin.kind != ObjectKind::Track
        || !origin.is_valid()
        || !matches!(kind, ObjectKind::Artist | ObjectKind::Album)
        || cancelled.load(Ordering::Relaxed)
    {
        return None;
    }
    let title = normalized(title);
    if title.is_empty() {
        return None;
    }
    let store = Store::open_default().ok()?;
    let query = PageQuery::resolve(&store, origin, profile, &region()).ok()?;
    resolve(&query, origin, kind, &title, cancelled)
}

fn resolve(
    query: &PageQuery<'_>,
    origin: &ObjectRef,
    kind: ObjectKind,
    title: &str,
    cancelled: &AtomicBool,
) -> Option<ObjectRef> {
    if let Ok(Some(page)) = query.cached() {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        match from_page(&page.data, origin, kind, title) {
            Relation::Unique(reference) => return Some(reference),
            Relation::Ambiguous => return None,
            Relation::Missing if page.fresh(ObjectKind::Track) => return None,
            Relation::Missing => {}
        }
    }
    let page = query.refresh(cancelled).ok()?;
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    match from_page(&page, origin, kind, title) {
        Relation::Unique(reference) => Some(reference),
        Relation::Ambiguous | Relation::Missing => None,
    }
}

#[derive(Debug, PartialEq)]
enum Relation {
    Unique(ObjectRef),
    Ambiguous,
    Missing,
}

fn from_page(page: &MusicPage, origin: &ObjectRef, kind: ObjectKind, title: &str) -> Relation {
    if page.reference.kind != ObjectKind::Track || page.reference.provider != origin.provider {
        return Relation::Missing;
    }
    // Some catalogues canonicalize the requested track ID, e.g. by adding a release ID.
    // A different row or a playback substitute cannot identify the recording's credits.
    let own_rows = page
        .tracks
        .iter()
        .filter(|track| track.id == page.reference.id);
    match kind {
        ObjectKind::Artist => unique_relation(
            page.artist_credits
                .iter()
                .chain(own_rows.flat_map(|track| &track.artist_credits))
                .map(|credit| (credit.name.as_str(), credit.reference.as_ref())),
            origin,
            kind,
            title,
        ),
        ObjectKind::Album => {
            unique_relation(
                page.album
                    .as_deref()
                    .map(|name| (name, page.album_ref.as_ref()))
                    .into_iter()
                    .chain(own_rows.filter_map(|track| {
                        Some((track.album.as_deref()?, track.album_ref.as_ref()))
                    })),
                origin,
                kind,
                title,
            )
        }
        _ => Relation::Missing,
    }
}

fn unique_relation<'a>(
    candidates: impl Iterator<Item = (&'a str, Option<&'a ObjectRef>)>,
    origin: &ObjectRef,
    kind: ObjectKind,
    title: &str,
) -> Relation {
    let mut known = None;
    let mut unlinked = false;
    for (_, reference) in candidates.filter(|(name, _)| normalized(name) == title) {
        let Some(reference) = reference.filter(|reference| {
            reference.provider == origin.provider && reference.kind == kind && reference.is_valid()
        }) else {
            unlinked = true;
            continue;
        };
        if known.is_some_and(|known| known != reference) {
            return Relation::Ambiguous;
        }
        known = Some(reference);
    }
    match known {
        Some(_) if unlinked && kind == ObjectKind::Artist => Relation::Ambiguous,
        Some(reference) => Relation::Unique(reference.clone()),
        None => Relation::Missing,
    }
}

#[cfg(test)]
mod tests;
