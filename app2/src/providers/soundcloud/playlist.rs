use std::{
    collections::{HashMap, HashSet},
    sync::atomic::AtomicBool,
};

use serde_json::Value;

use crate::music::{ObjectKind, ObjectRef};
use crate::playlist::{Media, Playlist, PlaylistOwner, PlaylistTrack};
use crate::providers::{
    ProviderError,
    playlists::{PlaylistPage, PlaylistProgress, unavailable_track},
    transport::*,
};

use super::{PROVIDER, get_with, item, resolve, stream::full_stream_allowed, valid_track_id};

const TRACK_BATCH: usize = 50;
const SYSTEM_PLAYLIST_PREFIX: &str = "soundcloud:system-playlists:";
const ARTIST_STATION_PREFIX: &str = "artist-stations:";

pub(crate) fn system_playlist_id(id: &str) -> bool {
    id.strip_prefix(SYSTEM_PLAYLIST_PREFIX).is_some_and(|rest| {
        !rest.is_empty()
            && rest.len() <= 256
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":-_".contains(&byte))
    })
}

pub(crate) fn personal_playlist_id(id: &str) -> bool {
    system_playlist_id(id)
        && id.strip_prefix(SYSTEM_PLAYLIST_PREFIX).is_some_and(|name| {
            matches!(
                name.split(':').next(),
                Some("personalized-tracks" | "your-moods" | "new-for-you" | "weekly")
            )
        })
}

pub(crate) fn artist_station_id(id: &str) -> bool {
    let Some(artist) = id
        .strip_prefix(SYSTEM_PLAYLIST_PREFIX)
        .and_then(|id| id.strip_prefix(ARTIST_STATION_PREFIX))
    else {
        return false;
    };
    match artist.split_once(':') {
        Some((artist, context)) => valid_track_id(artist) && valid_track_id(context),
        None => valid_track_id(artist),
    }
}

pub(crate) fn artist_station_from_slug(slug: &str) -> Option<String> {
    let id = format!("{SYSTEM_PLAYLIST_PREFIX}{slug}");
    artist_station_id(&id).then_some(id)
}

pub(crate) fn artist_station_url(id: &str) -> Option<String> {
    if !artist_station_id(id) {
        return None;
    }
    let slug = id.strip_prefix(SYSTEM_PLAYLIST_PREFIX)?;
    Some(format!("https://soundcloud.com/discover/sets/{slug}"))
}

pub(crate) fn valid_playlist_id(id: &str) -> bool {
    valid_track_id(id)
        || system_playlist_id(id)
        || item::permalink(id, ObjectKind::Playlist).is_some()
}

pub(crate) fn playlist(
    token: &str,
    client_id: &str,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<Playlist, ProviderError> {
    playlist_with_progress(token, client_id, id, cancelled, &mut |_| Ok(()))
}

pub(crate) fn playlist_with_progress(
    token: &str,
    client_id: &str,
    id: &str,
    cancelled: &AtomicBool,
    progress: &mut PlaylistProgress<'_>,
) -> Result<Playlist, ProviderError> {
    let agent = agent();
    if let Some(url) = item::permalink(id, ObjectKind::Playlist) {
        let value = resolve(&agent, &url, token, client_id)?;
        if value["kind"] != "playlist" {
            return Err(invalid_response(PROVIDER));
        }
        return from_response_with_progress(&agent, &value, token, client_id, cancelled, progress);
    }
    let endpoint = if system_playlist_id(id) {
        "system-playlists"
    } else {
        "playlists"
    };
    let value = get_with(
        &agent,
        &format!("/{endpoint}/{id}?representation=full"),
        token,
        client_id,
    )?;
    from_response_with_progress(&agent, &value, token, client_id, cancelled, progress)
}

pub(super) fn from_response(
    agent: &ureq::Agent,
    value: &Value,
    token: &str,
    client_id: &str,
    cancelled: &AtomicBool,
) -> Result<Playlist, ProviderError> {
    from_response_with_progress(agent, value, token, client_id, cancelled, &mut |_| Ok(()))
}

fn from_response_with_progress(
    agent: &ureq::Agent,
    value: &Value,
    token: &str,
    client_id: &str,
    cancelled: &AtomicBool,
    progress: &mut PlaylistProgress<'_>,
) -> Result<Playlist, ProviderError> {
    hydrate(
        value,
        cancelled,
        |batch| {
            let ids = batch
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            get_with(agent, &format!("/tracks?ids={ids}"), token, client_id)
        },
        progress,
    )
}

fn hydrate(
    value: &Value,
    cancelled: &AtomicBool,
    mut read_batch: impl FnMut(&[u64]) -> Result<Value, ProviderError>,
    progress: &mut PlaylistProgress<'_>,
) -> Result<Playlist, ProviderError> {
    check_cancelled(cancelled)?;
    let card = item::card(value).ok_or_else(|| invalid_response(PROVIDER))?;
    let album = item::is_album(value)
        .then(|| ObjectRef::from_item(PROVIDER, &card))
        .flatten();
    let entries = value["tracks"]
        .as_array()
        .or_else(|| value["track_ids"].as_array())
        .map(Vec::as_slice)
        .or_else(|| (value["track_count"] == 0).then_some(&[][..]))
        .ok_or_else(|| invalid_response(PROVIDER))?;
    let mut playlist = Playlist {
        name: card.title,
        owner: owner(value),
        artwork: card.artwork,
        link: card.url,
        tracks: Vec::with_capacity(entries.len()),
    };
    let mut batches = TrackBatches::default();
    while playlist.tracks.len() < entries.len() {
        check_cancelled(cancelled)?;
        let offset = playlist.tracks.len();
        for (index, entry) in entries.iter().enumerate().skip(offset) {
            let Some(row) = batches.resolve(entry) else {
                break;
            };
            let id =
                entry_id(entry).map_or_else(|| format!("missing:{index}"), |id| id.to_string());
            let mut track = track(row, id, field(entry, "added_at"));
            if let Some(album) = &album {
                track.album = Some(playlist.name.clone());
                track.album_ref = Some(album.clone());
            }
            playlist.tracks.push(track);
        }
        let received = playlist.tracks.len();
        if received == entries.len() {
            break;
        }
        if received > offset {
            progress(PlaylistPage {
                playlist: &playlist,
                tracks: &playlist.tracks[offset..],
                offset,
                total: Some(entries.len()),
            })?;
        }
        batches.load(&entries[received..], &mut read_batch)?;
    }
    Ok(playlist)
}

fn owner(value: &Value) -> Option<PlaylistOwner> {
    let user = &value["user"];
    let name = field(user, "username")?;
    let reference = user["id"]
        .as_u64()
        .filter(|id| {
            *id != 0
                && user["kind"].as_str().is_none_or(|kind| kind == "user")
                && value["user_id"].as_u64().is_none_or(|owner| owner == *id)
        })
        .and_then(|id| ObjectRef::native(PROVIDER, ObjectKind::Artist, id.to_string()));
    Some(PlaylistOwner::new(name, reference))
}

#[derive(Default)]
struct TrackBatches(HashMap<u64, Option<Value>>);

impl TrackBatches {
    fn resolve<'a>(&'a self, entry: &'a Value) -> Option<&'a Value> {
        match missing_id(entry) {
            Some(id) => Some(self.0.get(&id)?.as_ref().unwrap_or(entry)),
            None => Some(entry),
        }
    }

    fn load(
        &mut self,
        entries: &[Value],
        read_batch: &mut impl FnMut(&[u64]) -> Result<Value, ProviderError>,
    ) -> Result<(), ProviderError> {
        let mut seen = HashSet::new();
        let batch = entries
            .iter()
            .filter_map(missing_id)
            .filter(|id| !self.0.contains_key(id) && seen.insert(*id))
            .take(TRACK_BATCH)
            .collect::<Vec<_>>();
        let data = read_batch(&batch)?;
        let tracks = data
            .as_array()
            .or_else(|| data["collection"].as_array())
            .ok_or_else(|| invalid_response(PROVIDER))?;
        self.0.extend(batch.into_iter().map(|id| (id, None)));
        for track in tracks {
            if let Some(row) = entry_id(track).and_then(|id| self.0.get_mut(&id)) {
                *row = Some(track.clone());
            }
        }
        Ok(())
    }
}

fn missing_id(entry: &Value) -> Option<u64> {
    entry["title"].is_null().then(|| entry_id(entry)).flatten()
}

fn entry_id(value: &Value) -> Option<u64> {
    value["id"].as_u64().or_else(|| value.as_u64())
}

pub(super) fn track(row: &Value, id: String, added: Option<String>) -> PlaylistTrack {
    let Some(card) = item::track(row) else {
        return PlaylistTrack {
            added,
            ..unavailable_track(id)
        };
    };
    let artist_credit = item::artist_credit(row);
    PlaylistTrack {
        media: (valid_track_id(&id) && full_stream_allowed(row)).then(|| Media::Provider {
            provider: PROVIDER,
            id: id.clone(),
        }),
        id,
        title: card.title,
        artist: artist_credit
            .as_ref()
            .map_or(card.subtitle, |credit| credit.name.clone()),
        artists: Vec::new(),
        artist_credits: artist_credit.into_iter().collect(),
        album_ref: None,
        album: field(&row["publisher_metadata"], "album_title"),
        artwork: card.artwork,
        added,
        duration: row["duration"]
            .as_u64()
            .and_then(|ms| u32::try_from(ms / 1000).ok()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_the_embedded_playlist_owner_to_the_exact_user() {
        let mut response = serde_json::json!({
            "kind":"playlist", "id":19, "title":"Public set", "tracks":[],
            "user_id":20, "user":{"kind":"user", "username":"Set author", "id":20}
        });
        let load = |value: &Value| {
            from_response(&agent(), value, "", "", &AtomicBool::new(false))
                .unwrap()
                .owner
                .unwrap()
        };
        let author = load(&response);
        assert_eq!(author.name(), "Set author");
        assert_eq!(
            author.reference(),
            ObjectRef::native(PROVIDER, ObjectKind::Artist, "20".into()).as_ref()
        );
        response["user_id"] = serde_json::json!(30);
        assert!(load(&response).reference().is_none());
        response["user"]["id"] = Value::Null;
        assert_eq!(load(&response), PlaylistOwner::Named("Set author".into()));
    }

    #[test]
    fn soundcloud_tracks_link_a_confirmed_album_but_keep_plain_publisher_titles() {
        let mut value = serde_json::json!({
            "kind": "playlist", "id": 123, "title": "Official album", "is_album": true,
            "tracks": [
                {"kind": "track", "id": 456, "title": "Recording",
                 "publisher_metadata": {"album_title": "Publisher title"}}
            ]
        });
        let agent = agent();
        let cancelled = AtomicBool::new(false);
        let load = |set: &Value| from_response(&agent, set, "", "", &cancelled).unwrap();
        let album = load(&value);
        assert_eq!(album.tracks[0].album.as_deref(), Some("Official album"));
        let reference = album.tracks[0].album_ref.as_ref().unwrap();
        assert_eq!(reference.kind, ObjectKind::Album);
        assert_eq!(reference.id, "123");

        value["is_album"] = serde_json::json!(false);
        let playlist = load(&value);
        assert_eq!(playlist.tracks[0].album.as_deref(), Some("Publisher title"));
        assert!(playlist.tracks[0].album_ref.is_none());
    }

    #[test]
    fn publishes_a_contiguous_prefix_before_fetching_each_missing_batch() {
        use std::cell::RefCell;

        let mut entries =
            vec![serde_json::json!({"kind":"track", "id":1, "title":"Received inline"})];
        entries.extend((2..=TRACK_BATCH as u64 + 2).map(|id| serde_json::json!(id)));
        entries.push(serde_json::json!(2));
        let playlist =
            serde_json::json!({"kind":"playlist", "id":123, "title":"Playlist", "tracks":entries});
        let events = RefCell::new(Vec::new());
        let result = hydrate(
            &playlist,
            &AtomicBool::new(false),
            |ids| {
                events
                    .borrow_mut()
                    .push(("request", ids[0] as usize, ids.len()));
                Ok(Value::Array(
                    ids.iter()
                        .skip(1)
                        .map(|id| {
                            serde_json::json!({
                                "kind": "track",
                                "id": id,
                                "title": format!("Track {id}"),
                            })
                        })
                        .collect(),
                ))
            },
            &mut |page| {
                events
                    .borrow_mut()
                    .push(("page", page.offset, page.tracks.len()));
                assert_eq!(page.total, Some(entries.len()));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            events.into_inner(),
            [
                ("page", 0, 1),
                ("request", 2, TRACK_BATCH),
                ("page", 1, TRACK_BATCH),
                ("request", TRACK_BATCH + 2, 1),
            ]
        );
        assert_eq!(result.tracks.len(), entries.len());
        assert_eq!(result.tracks[0].title, "Received inline");
        assert_eq!(result.tracks[1].id, result.tracks.last().unwrap().id);
        assert!(result.tracks[1].media.is_none());
    }
}
