use std::{collections::HashSet, sync::atomic::AtomicBool};

use serde_json::{Value, json};
use ureq::Agent;

use crate::{
    music::{ArtistDetails, MusicPage, ObjectKind, ObjectRef},
    playlist::PlaylistTrack,
    providers::{Credentials, ErrorKind, HomeItem, ProviderError, catalog, transport::*},
};

use super::{
    Auth, PROVIDER, album_ref, album_run, artist_credits, artist_endpoint, artist_names, browse_id,
    columns, complete_artist_names, feed, named, playlist, request_with_agent, runs, text,
};

pub(in crate::providers) fn read(
    reference: &ObjectRef,
    credentials: Option<&Credentials>,
    cancelled: &AtomicBool,
) -> Result<MusicPage, ProviderError> {
    let id = reference.id.as_str();
    if !feed::safe_id(id) {
        return Err(catalog::invalid_id(PROVIDER));
    }
    let client = Client::new(credentials);
    match reference.kind {
        ObjectKind::Track if id.len() == 11 => read_track(&client, reference, cancelled),
        ObjectKind::Album if id.starts_with("MPRE") => read_album(&client, reference, cancelled),
        ObjectKind::Album if id.starts_with("OLAK5uy_") => {
            let data = client.request(
                "browse",
                json!({"browseId": format!("VL{id}")}),
                None,
                cancelled,
            )?;
            let id = album_reference(&data).ok_or_else(|| invalid_response(PROVIDER))?;
            read_album(
                &client,
                &ObjectRef {
                    id,
                    ..reference.clone()
                },
                cancelled,
            )
        }
        ObjectKind::Artist if id.starts_with("UC") => read_artist(&client, reference, cancelled),
        _ => Err(catalog::invalid_id(PROVIDER)),
    }
}

pub(in crate::providers) fn search(
    credentials: &Credentials,
    kind: ObjectKind,
    query: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<MusicPage>, ProviderError> {
    let client = Client::new(Some(credentials));
    search_with(&client, kind, query, cancelled)
}

fn search_with(
    client: &Client<'_>,
    kind: ObjectKind,
    query: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<MusicPage>, ProviderError> {
    // The WEB_REMIX filters select song, album, artist and playlist shelves.
    let params = match kind {
        ObjectKind::Track => "EgWKAQIIAQ%3D%3D",
        ObjectKind::Album => "EgWKAQIYAQ%3D%3D",
        ObjectKind::Artist => "EgWKAQIgAQ%3D%3D",
        ObjectKind::Playlist => "EgWKAQIoAQ%3D%3D",
        ObjectKind::Collection | ObjectKind::Station => return Err(catalog::invalid_id(PROVIDER)),
    };
    let data = client.request(
        "search",
        json!({"query": query, "params": params}),
        None,
        cancelled,
    )?;
    let mut shelves = Vec::new();
    named(&data, "musicShelfRenderer", &mut shelves);
    let mut seen = HashSet::new();
    Ok(shelves
        .into_iter()
        .flat_map(|shelf| shelf["contents"].as_array().into_iter().flatten())
        .filter_map(|row| search_entry(row, kind))
        .filter(|page| seen.insert(page.reference.id.clone()))
        .take(catalog::SEARCH_LIMIT)
        .collect())
}

pub(super) struct Client<'a> {
    agent: Agent,
    auth: Option<Auth<'a>>,
}

impl<'a> Client<'a> {
    pub(super) fn new(credentials: Option<&'a Credentials>) -> Self {
        Self {
            agent: agent(),
            auth: credentials.map(Auth::from_credentials),
        }
    }

    pub(super) fn request(
        &self,
        endpoint: &str,
        mut body: Value,
        visitor: Option<&str>,
        cancelled: &AtomicBool,
    ) -> Result<Value, ProviderError> {
        check_cancelled(cancelled)?;
        let region = crate::music::region();
        if region != "ZZ" {
            body["context"]["client"]["gl"] = region.into();
        }
        let value =
            request_with_agent(&self.agent, false, endpoint, body, self.auth, visitor, None)?;
        check_cancelled(cancelled)?;
        playlist::check_access(self.auth, &value)?;
        Ok(value)
    }
}

fn read_album(
    client: &Client<'_>,
    reference: &ObjectRef,
    cancelled: &AtomicBool,
) -> Result<MusicPage, ProviderError> {
    let data = client.request("browse", json!({"browseId": reference.id}), None, cancelled)?;
    let (header, title) = header(
        &data,
        &["musicResponsiveHeaderRenderer", "musicDetailHeaderRenderer"],
    )
    .ok_or_else(|| invalid_response(PROVIDER))?;
    let mut page = MusicPage::new(reference.clone(), title);
    page.artist_credits = artist_credits(&header["straplineTextOne"]).collect();
    if page.artist_credits.is_empty() {
        page.artist_credits = artist_credits(&header["subtitle"]).collect();
    }
    page.artists = artist_names(&page.artist_credits);
    let artists = complete_artist_names(&page.artist_credits).unwrap_or_default();
    page.artwork = playlist::artwork(header);
    page.year = year(&header["subtitle"]);
    page.single = has_single_label(&header["subtitle"]);
    let visitor = field(&data["responseContext"], "visitorData");
    page.tracks = playlist::load_tracks(data, cancelled, |token| {
        client.request(
            "browse",
            json!({"continuation": token}),
            visitor.as_deref(),
            cancelled,
        )
    })?;
    // Album browse omits artists on tracks credited to the release artist.
    let credit = page.artists.join(", ");
    for track in &mut page.tracks {
        if track.artist.is_empty() {
            track.artist.clone_from(&credit);
            track.artists.clone_from(&artists);
            track.artist_credits.clone_from(&page.artist_credits);
        }
        track.album = Some(page.title.clone());
        track.album_ref = Some(page.reference.clone());
    }
    Ok(page)
}

fn read_artist(
    client: &Client<'_>,
    reference: &ObjectRef,
    cancelled: &AtomicBool,
) -> Result<MusicPage, ProviderError> {
    let data = client.request("browse", json!({"browseId": reference.id}), None, cancelled)?;
    let (header, title) = header(
        &data,
        &[
            "musicImmersiveHeaderRenderer",
            "musicVisualHeaderRenderer",
            "musicResponsiveHeaderRenderer",
        ],
    )
    .ok_or_else(|| invalid_response(PROVIDER))?;
    let mut page = MusicPage::new(reference.clone(), title);
    page.artwork = playlist::artwork(header);
    let mut shelves = Vec::new();
    named(&data, "musicCarouselShelfRenderer", &mut shelves);
    page.artist_details = Some(artist_details(header, &shelves, reference));
    let mut seen = HashSet::new();
    page.releases = shelves
        .iter()
        .flat_map(|shelf| shelf["contents"].as_array().into_iter().flatten())
        .filter_map(|row| feed::home_item(row, "Релиз"))
        .filter(|card| card.id.starts_with("browse:MPRE") && seen.insert(card.id.clone()))
        .collect();
    let mut songs = Vec::new();
    named(&data, "musicShelfRenderer", &mut songs);
    if let Some(shelf) = songs.first() {
        page.tracks = shelf["contents"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row.get("musicResponsiveListItemRenderer"))
            .enumerate()
            .map(|(position, row)| playlist::track(row, position))
            .filter(|row| !row.id.starts_with("missing:"))
            .collect();
    }
    Ok(page)
}

fn artist_details(header: &Value, shelves: &[&Value], reference: &ObjectRef) -> ArtistDetails {
    let related = shelves
        .iter()
        .filter(|shelf| {
            let title = text(&shelf["header"]["musicCarouselShelfBasicHeaderRenderer"]["title"]);
            matches!(
                title.as_deref(),
                Some("Похожие исполнители" | "Fans might also like")
            )
        })
        .flat_map(|shelf| shelf["contents"].as_array().into_iter().flatten())
        .filter_map(|row| {
            let renderer = row
                .get("musicTwoRowItemRenderer")
                .or_else(|| row.get("musicResponsiveListItemRenderer"))?;
            artist_endpoint(&renderer["navigationEndpoint"]["browseEndpoint"])?;
            feed::home_item(row, "Исполнитель")
        });
    ArtistDetails {
        biography: text(&header["description"]),
        monthly_listeners_caption: text(&header["monthlyListenerCount"]),
        related: ArtistDetails::related_preview(reference, related),
        ..Default::default()
    }
}

fn read_track(
    client: &Client<'_>,
    reference: &ObjectRef,
    cancelled: &AtomicBool,
) -> Result<MusicPage, ProviderError> {
    let data = client.request("next", json!({"videoId": reference.id}), None, cancelled)?;
    let mut rows = Vec::new();
    named(&data, "playlistPanelVideoRenderer", &mut rows);
    let row = rows
        .into_iter()
        .find(|row| row["videoId"].as_str() == Some(reference.id.as_str()))
        .ok_or_else(|| {
            ProviderError::new(
                ErrorKind::Unavailable,
                "YouTube Music не открыл эту запись.",
            )
        })?;
    let subtitle = &row["longBylineText"];
    let mut page = MusicPage::new(
        reference.clone(),
        text(&row["title"]).ok_or_else(|| invalid_response(PROVIDER))?,
    );
    page.artist_credits = artist_credits(subtitle).collect();
    page.artists = artist_names(&page.artist_credits);
    let album = album_run(subtitle);
    page.album = album.and_then(|run| field(run, "text"));
    page.album_ref = album.and_then(album_ref);
    page.year = year(subtitle);
    page.artwork = artwork_from_images(PROVIDER, &row["thumbnail"]["thumbnails"]);
    page.duration = text(&row["lengthText"])
        .as_deref()
        .and_then(playlist::duration_seconds)
        .and_then(|seconds| seconds.checked_mul(1000));
    page.tracks.push(PlaylistTrack {
        id: reference.id.clone(),
        title: page.title.clone(),
        artist: page.artists.join(", "),
        artists: complete_artist_names(&page.artist_credits).unwrap_or_default(),
        artist_credits: page.artist_credits.clone(),
        album_ref: page.album_ref.clone(),
        album: page.album.clone(),
        artwork: page.artwork.clone(),
        added: None,
        duration: page.duration.map(|ms| ms / 1000),
        media: None,
    });
    Ok(page)
}

fn album_reference(data: &Value) -> Option<String> {
    let mut shelves = Vec::new();
    named(data, "musicPlaylistShelfRenderer", &mut shelves);
    let mut albums = shelves
        .iter()
        .flat_map(|shelf| shelf["contents"].as_array().into_iter().flatten())
        .filter_map(|row| row.get("musicResponsiveListItemRenderer"))
        .flat_map(|row| columns(row).filter_map(album_run))
        .filter_map(browse_id);
    let first = albums.next()?;
    albums.all(|id| id == first).then(|| first.to_owned())
}

fn header<'a>(root: &'a Value, kinds: &[&str]) -> Option<(&'a Value, String)> {
    let mut headers = Vec::new();
    for kind in kinds {
        named(root, kind, &mut headers);
    }
    headers
        .into_iter()
        .find_map(|node| Some((node, text(&node["title"])?)))
}

fn search_entry(entry: &Value, kind: ObjectKind) -> Option<MusicPage> {
    let HomeItem {
        id, title, artwork, ..
    } = feed::home_item(entry, "")?;
    let matched = match kind {
        ObjectKind::Artist => id.strip_prefix("browse:").filter(|id| id.starts_with("UC")),
        ObjectKind::Album => id
            .strip_prefix("browse:")
            .filter(|id| id.starts_with("MPRE")),
        ObjectKind::Track => id.strip_prefix("video:"),
        ObjectKind::Playlist => id.strip_prefix("browse:VL"),
        ObjectKind::Collection | ObjectKind::Station => None,
    }?;
    let mut page = MusicPage::new(
        ObjectRef {
            provider: PROVIDER,
            kind,
            id: matched.to_owned(),
        },
        title,
    );
    page.artwork = artwork;
    let row = &entry["musicResponsiveListItemRenderer"];
    if !row.is_null() {
        let fields = columns(row);
        page.artist_credits = fields.clone().flat_map(artist_credits).collect();
        page.artists = artist_names(&page.artist_credits);
        page.year = fields.clone().find_map(year);
        page.single = fields.clone().any(has_single_label);
        let album = fields.clone().find_map(album_run);
        page.album = album.and_then(|run| field(run, "text"));
        page.album_ref = album.and_then(album_ref);
        if kind == ObjectKind::Track {
            let mut track = playlist::track(row, 0);
            track.id = matched.to_owned();
            track.album.clone_from(&page.album);
            track.album_ref.clone_from(&page.album_ref);
            if !page.artists.is_empty() {
                track.artist = page.artists.join(", ");
                track.artists = complete_artist_names(&page.artist_credits).unwrap_or_default();
                track.artist_credits.clone_from(&page.artist_credits);
            }
            track.duration = track.duration.or_else(|| {
                fields
                    .flat_map(runs)
                    .filter_map(|run| run["text"].as_str())
                    .find_map(playlist::duration_seconds)
            });
            page.duration = track.duration.and_then(|seconds| seconds.checked_mul(1000));
            page.tracks.push(track);
        }
    }
    Some(page)
}

fn year(value: &Value) -> Option<u32> {
    runs(value)
        .filter_map(|run| run["text"].as_str()?.parse::<u32>().ok())
        .find(|year| (1800..=2100).contains(year))
}

fn has_single_label(value: &Value) -> bool {
    runs(value).any(|run| {
        run["text"]
            .as_str()
            .is_some_and(|label| matches!(label, "Single" | "Сингл" | "сингл"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_typed_artists_in_the_explicit_similar_shelf() {
        let card = |id: &str, kind: &str| {
            json!({"musicTwoRowItemRenderer": {
                "title":{"runs":[{"text":"Related artist"}]},
                "navigationEndpoint":{"browseEndpoint":{
                    "browseId":id,
                    "browseEndpointContextSupportedConfigs":{
                        "browseEndpointContextMusicConfig":{"pageType":kind}
                    }
                }}
            }})
        };
        let make_shelf = |title: &str, contents: Vec<Value>| {
            json!({
                "header":{"musicCarouselShelfBasicHeaderRenderer":{"title":{"simpleText":title}}},
                "contents":contents
            })
        };
        let typed = "MUSIC_PAGE_TYPE_ARTIST";
        let origin = ObjectRef::native(PROVIDER, ObjectKind::Artist, "UCOrigin".into()).unwrap();
        let unrelated = make_shelf("Где встречается", vec![card("UCWrongShelf", typed)]);
        let similar = make_shelf(
            "Похожие исполнители",
            vec![
                card("UCOrigin", typed),
                card("UCAuthor", "MUSIC_PAGE_TYPE_USER_CHANNEL"),
                card("UCOther", typed),
                card("UCOther", typed),
            ],
        );
        let caption = "55,9\u{a0}млн слушателей в месяц";
        let header = json!({"description":{"simpleText":"Biography"}, "monthlyListenerCount":{"simpleText":caption}});
        let details = artist_details(&header, &[&unrelated, &similar], &origin);
        assert_eq!(
            details
                .related
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["browse:UCOther"]
        );
        assert_eq!(details.biography.as_deref(), Some("Biography"));
        assert_eq!(details.monthly_listeners_caption.as_deref(), Some(caption));
        assert!(details.monthly_listeners.is_none());
    }

    #[test]
    #[ignore = "public YouTube Music catalog"]
    fn youtube_searches_and_reads_a_catalog_album() {
        let cancelled = AtomicBool::new(false);
        let client = Client {
            agent: agent(),
            auth: None,
        };
        let hits = search_with(
            &client,
            ObjectKind::Album,
            "Daft Punk Random Access Memories",
            &cancelled,
        )
        .unwrap();
        let first = hits
            .iter()
            .find(|page| page.title == "Random Access Memories")
            .unwrap();
        let page = read(&first.reference, None, &cancelled).unwrap();
        assert_eq!(page.title, "Random Access Memories");
        assert!(
            page.tracks
                .iter()
                .any(|track| track.title.contains("Get Lucky"))
        );
        let alias = ObjectRef {
            id: "OLAK5uy_kNhM2yaBTOVwrcZJepB1C9P3-n5_Sfy5c".into(),
            ..first.reference.clone()
        };
        let via_alias = read(&alias, None, &cancelled).unwrap();
        assert_eq!(via_alias.reference, page.reference);
        assert_eq!(via_alias.tracks.len(), page.tracks.len());
    }

    #[test]
    #[ignore = "public YouTube Music catalog"]
    fn youtube_reads_a_catalog_recording_with_artist_credits() {
        let cancelled = AtomicBool::new(false);
        let reference = ObjectRef {
            provider: PROVIDER,
            kind: ObjectKind::Track,
            id: "4D7u5KF7SP8".into(),
        };
        let track = read(&reference, None, &cancelled).unwrap();
        assert!(track.title.contains("Get Lucky"));
        assert_eq!(
            track.artists,
            ["Daft Punk", "Pharrell Williams", "Nile Rodgers"]
        );
        assert_eq!(track.tracks[0].artists, track.artists);
        assert_eq!(track.album.as_deref(), Some("Random Access Memories"));
        assert_eq!(track.duration, Some(370_000));
        let credit = &track.artist_credits[0];
        let artist = read(credit.reference.as_ref().unwrap(), None, &cancelled).unwrap();
        assert_eq!(artist.title, credit.name);
        let album = read(track.album_ref.as_ref().unwrap(), None, &cancelled).unwrap();
        assert_eq!(track.album.as_deref(), Some(album.title.as_str()));
    }

    #[test]
    #[ignore = "public YouTube Music catalog"]
    fn youtube_artist_search_opens_discography() {
        let cancelled = AtomicBool::new(false);
        let client = Client {
            agent: agent(),
            auth: None,
        };
        let hits = search_with(&client, ObjectKind::Artist, "Daft Punk", &cancelled).unwrap();
        let hit = hits.iter().find(|page| page.title == "Daft Punk").unwrap();
        let artist = read(&hit.reference, None, &cancelled).unwrap();
        assert_eq!(artist.title, hit.title);
        assert!(!artist.releases.is_empty());
        assert!(!artist.tracks.is_empty());
    }
}
