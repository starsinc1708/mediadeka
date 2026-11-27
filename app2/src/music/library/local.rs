use std::collections::{BTreeMap, HashSet};

use crate::{
    music::likes::LikeIdentity,
    playlist::{Media, PlaylistKey},
    providers::{HomeItem, collections::CollectionKind},
    storage::{FavoriteScope, LocalPlaylistSummary, Store, StoreError},
};

use super::{Entry, EntryAction, EntryId, EntryKey, LibraryGroup, Scope};

pub(super) fn read(store: &Store, kind: CollectionKind) -> Result<Vec<Entry>, StoreError> {
    if kind == CollectionKind::Playlists {
        return Ok(Vec::new());
    }
    let local = store.local_library_tracks()?;
    if kind == CollectionKind::Liked {
        let liked = store.local_liked_tracks()?;
        let marked = store.favorite_tracks(FavoriteScope::Local)?;
        let saved_files: HashSet<_> = liked
            .iter()
            .filter(|track| matches!(track.media, Some(Media::File(_))))
            .map(|track| track.id.as_str())
            .collect();
        let mut files = HashSet::new();
        let mut entries = Vec::with_capacity(local.len() + liked.len());
        for (id, track) in local {
            files.insert(track.id.clone());
            let liked = marked.contains(&track.id) || saved_files.contains(track.id.as_str());
            entries.push(Entry::track(
                Scope::Local,
                PlaylistKey::Local(id),
                track,
                liked,
            ));
        }
        for track in liked {
            if files.contains(&track.id) {
                continue;
            }
            entries.push(saved_entry(track));
        }
        return Ok(entries);
    }
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for (_, track) in &local {
        let group = match kind {
            CollectionKind::Albums => {
                let Some(title) = track.album.as_ref().filter(|album| !album.is_empty()) else {
                    continue;
                };
                LibraryGroup::Album {
                    title: title.clone(),
                    artist: track.artist.clone(),
                }
            }
            CollectionKind::Artists if !track.artist.is_empty() => {
                LibraryGroup::Artist(track.artist.clone())
            }
            _ => continue,
        };
        groups.entry(group).or_default().push(track);
    }
    Ok(groups
        .into_iter()
        .map(|(group, tracks)| {
            let item = HomeItem {
                id: String::new(),
                title: group.title().to_owned(),
                subtitle: match &group {
                    LibraryGroup::Album { artist, .. } => artist.clone(),
                    LibraryGroup::Artist(_) => String::new(),
                },
                artwork: None,
                url: None,
            };
            Entry::item(
                EntryKey(Scope::Local, EntryId::LocalGroup(group.clone())),
                &item,
                tracks
                    .iter()
                    .filter_map(|track| track.added.as_deref())
                    .min(),
                tracks
                    .iter()
                    .any(|track| matches!(&track.media, Some(Media::File(path)) if path.is_file())),
                EntryAction::LocalGroup(group),
            )
        })
        .collect())
}

pub(in crate::music) fn like_change(
    store: &Store,
    identity: &LikeIdentity,
) -> Result<super::LikeChange, StoreError> {
    let (id, saved) = store.local_liked_track(identity)?;
    let imported = if identity.provider.is_none() {
        store.local_library_track(&identity.id)?
    } else {
        None
    };
    let entry = if let Some((playlist, track)) = imported {
        Some(Entry::track(
            Scope::Local,
            PlaylistKey::Local(playlist),
            track,
            saved.is_some(),
        ))
    } else {
        saved.map(saved_entry)
    };
    Ok(super::LikeChange {
        key: EntryKey(Scope::Local, EntryId::Source(id)),
        entry,
        link: None,
    })
}

fn saved_entry(track: crate::playlist::PlaylistTrack) -> Entry {
    let key = LikeIdentity::from_track(&PlaylistKey::Local(0), &track)
        .and_then(|identity| {
            Some(PlaylistKey::Source {
                provider: identity.provider?,
                profile: identity.profile,
                id: format!("track:{}", identity.id),
            })
        })
        .unwrap_or(PlaylistKey::Local(0));
    Entry::track(Scope::Local, key, track, true)
}

pub(crate) fn playlist_entry(playlist: &LocalPlaylistSummary) -> Entry {
    let item = HomeItem {
        id: String::new(),
        title: playlist.name.clone(),
        subtitle: String::new(),
        artwork: None,
        url: None,
    };
    Entry::item(
        EntryKey(Scope::Local, EntryId::LocalPlaylist(playlist.id)),
        &item,
        playlist.added.as_deref(),
        playlist.has_files,
        EntryAction::LocalPlaylist {
            id: playlist.id,
            track_count: playlist.track_count,
        },
    )
}
