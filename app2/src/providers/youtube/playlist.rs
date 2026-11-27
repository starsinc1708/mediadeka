use std::{collections::HashSet, sync::atomic::AtomicBool};

use serde_json::{Value, json};

use crate::playlist::{Playlist, PlaylistOwner, PlaylistTrack};
use crate::providers::{
    Credentials, ErrorKind, ProviderError,
    playlists::{PlaylistPage, PlaylistProgress, unavailable_track},
    transport::*,
};

use super::{
    Auth, MUSIC_ORIGIN, PROVIDER, album_ref, album_run, artist_credits, artist_endpoint, columns,
    complete_artist_names, feed::safe_id, named, request_with_agent, runs, signed_out, text,
    valid_browse_token,
};

pub(crate) fn valid_playlist_id(id: &str) -> bool {
    safe_id(id)
}

pub(crate) fn playlist(
    credentials: Option<&Credentials>,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<Playlist, ProviderError> {
    playlist_with_progress(credentials, id, cancelled, &mut |_| Ok(()))
}

pub(crate) fn playlist_with_progress(
    credentials: Option<&Credentials>,
    id: &str,
    cancelled: &AtomicBool,
    progress: &mut PlaylistProgress<'_>,
) -> Result<Playlist, ProviderError> {
    let auth = credentials.map(Auth::from_credentials);
    let agent = agent();
    let data = request_with_agent(
        &agent,
        false,
        "browse",
        json!({"browseId": format!("VL{id}")}),
        auth,
        None,
        None,
    )?;
    check_access(auth, &data)?;
    let visitor = field(&data["responseContext"], "visitorData");
    let mut headers = Vec::new();
    named(&data, "musicResponsiveHeaderRenderer", &mut headers);
    named(&data, "musicDetailHeaderRenderer", &mut headers);
    let Some((header, name)) = headers
        .iter()
        .find_map(|header| Some((*header, text(&header["title"])?)))
    else {
        return Err(ProviderError::new(
            ErrorKind::Unavailable,
            "YouTube Music не открыл этот плейлист для текущего профиля.",
        ));
    };
    let mut playlist = Playlist {
        name,
        owner: owner(header),
        artwork: artwork(header),
        link: Some(format!("{MUSIC_ORIGIN}/playlist?list={id}")),
        tracks: Vec::new(),
    };
    playlist.tracks = load_track_pages(
        data,
        cancelled,
        |token| {
            let next = request_with_agent(
                &agent,
                false,
                "browse",
                json!({"continuation": token}),
                auth,
                visitor.as_deref(),
                None,
            )?;
            check_access(auth, &next)?;
            Ok(next)
        },
        |tracks, offset| {
            progress(PlaylistPage {
                playlist: &playlist,
                tracks,
                offset,
                total: None,
            })
        },
    )?;
    Ok(playlist)
}

fn owner(header: &Value) -> Option<PlaylistOwner> {
    let stack = &header["facepile"]["avatarStackViewModel"];
    if let Some(name) = field(&stack["text"], "content") {
        let endpoint = &stack["rendererContext"]["commandContext"]["onTap"]["innertubeCommand"]["browseEndpoint"];
        let reference = stack["avatars"]
            .as_array()
            .filter(|avatars| avatars.len() == 1)
            .and_then(|_| artist_endpoint(endpoint));
        return Some(PlaylistOwner::new(name, reference));
    }

    let byline = &header["straplineTextOne"];
    let documented = |run: &Value| {
        let endpoint = &run["navigationEndpoint"]["browseEndpoint"];
        let channel = endpoint["browseId"].as_str()?;
        if !(channel.starts_with("UC") && safe_id(channel)) {
            return None;
        }
        let name = field(run, "text")?;
        Some(PlaylistOwner::new(name, artist_endpoint(endpoint)))
    };
    runs(byline)
        .find_map(documented)
        .or_else(|| text(byline).map(PlaylistOwner::Named))
        .or_else(|| runs(&header["subtitle"]).find_map(documented))
}

pub(super) fn check_access(auth: Option<Auth<'_>>, data: &Value) -> Result<(), ProviderError> {
    if auth.is_some() && signed_out(data) {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "Войдите в YouTube Music снова.",
        ));
    }
    Ok(())
}

pub(super) fn load_tracks(
    data: Value,
    cancelled: &AtomicBool,
    next_page: impl FnMut(&str) -> Result<Value, ProviderError>,
) -> Result<Vec<PlaylistTrack>, ProviderError> {
    load_track_pages(data, cancelled, next_page, |_, _| Ok(()))
}

fn load_track_pages(
    mut data: Value,
    cancelled: &AtomicBool,
    mut next_page: impl FnMut(&str) -> Result<Value, ProviderError>,
    mut progress: impl FnMut(&[PlaylistTrack], usize) -> Result<(), ProviderError>,
) -> Result<Vec<PlaylistTrack>, ProviderError> {
    let mut tracks = Vec::new();
    let mut seen = HashSet::new();
    loop {
        check_cancelled(cancelled)?;
        let (shelves, entries) = entries(&data);
        let offset = tracks.len();
        tracks.extend(
            entries
                .iter()
                .filter_map(|row| row.get("musicResponsiveListItemRenderer"))
                .enumerate()
                .map(|(index, row)| track(row, offset + index)),
        );
        let next = shelves
            .into_iter()
            .find_map(|shelf| {
                shelf["continuations"]
                    .as_array()?
                    .iter()
                    .find_map(|entry| entry["nextContinuationData"]["continuation"].as_str())
            })
            .or_else(|| {
                entries.iter().find_map(|entry| {
                    entry["continuationItemRenderer"]["continuationEndpoint"]["continuationCommand"]
                        ["token"]
                        .as_str()
                })
            });
        let Some(next) = next else {
            break;
        };
        if !valid_browse_token(next) || (!seen.is_empty() && offset == tracks.len()) {
            return Err(invalid_response(PROVIDER));
        }
        // Mixes can include tracks along with a cursor we've already fetched.
        if !seen.insert(next.to_owned()) {
            break;
        }
        if offset < tracks.len() {
            check_cancelled(cancelled)?;
            progress(&tracks[offset..], offset)?;
        }
        data = next_page(next)?;
    }
    Ok(tracks)
}

fn entries(answer: &Value) -> (Vec<&Value>, Vec<&Value>) {
    let mut shelves = Vec::new();
    for key in [
        "musicPlaylistShelfRenderer",
        "musicPlaylistShelfContinuation",
        "musicShelfContinuation",
    ] {
        named(answer, key, &mut shelves);
    }
    if shelves.is_empty() {
        named(answer, "musicShelfRenderer", &mut shelves);
    }
    let mut items = shelves
        .iter()
        .flat_map(|shelf| shelf["contents"].as_array().into_iter().flatten())
        .collect::<Vec<_>>();
    if items.is_empty() {
        let mut actions = Vec::new();
        named(answer, "appendContinuationItemsAction", &mut actions);
        items.extend(
            actions
                .into_iter()
                .flat_map(|action| action["continuationItems"].as_array().into_iter().flatten()),
        );
    }
    (shelves, items)
}

pub(super) fn artwork(value: &Value) -> Option<String> {
    let mut pictures = Vec::new();
    named(value, "musicThumbnailRenderer", &mut pictures);
    pictures
        .into_iter()
        .find_map(|image| artwork_from_images(PROVIDER, &image["thumbnail"]["thumbnails"]))
}

pub(super) fn track(row: &Value, position: usize) -> PlaylistTrack {
    let id = field(&row["playlistItemData"], "videoId")
        .filter(|id| id.len() == 11 && safe_id(id))
        .unwrap_or_else(|| format!("missing:{position}"));
    let column = |index: usize| {
        &row["flexColumns"][index]["musicResponsiveListItemFlexColumnRenderer"]["text"]
    };
    let Some(title) = text(column(0)) else {
        return unavailable_track(id);
    };
    let known_album = columns(row).find_map(album_run);
    let artist_credits = columns(row).flat_map(artist_credits).collect::<Vec<_>>();
    PlaylistTrack {
        id,
        title,
        artist: text(column(1)).unwrap_or_default(),
        artists: complete_artist_names(&artist_credits).unwrap_or_default(),
        artist_credits,
        album_ref: known_album.and_then(album_ref),
        album: known_album
            .and_then(|run| field(run, "text"))
            .or_else(|| text(column(2))),
        artwork: artwork(row),
        added: None,
        duration: text(
            &row["fixedColumns"][0]["musicResponsiveListItemFixedColumnRenderer"]["text"],
        )
        .as_deref()
        .and_then(duration_seconds),
        media: None,
    }
}

pub(super) fn duration_seconds(value: &str) -> Option<u32> {
    if !value.contains(':') {
        return None;
    }
    value.split(':').try_fold(0u32, |total, component| {
        total.checked_mul(60)?.checked_add(component.parse().ok()?)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::{ObjectKind, ObjectRef};

    #[test]
    fn author_link_requires_an_explicit_artist_endpoint() {
        let header = |page_type: &str| {
            json!({"straplineTextOne":{"runs":[{
                "text":"Playlist author",
                "navigationEndpoint":{"browseEndpoint":{
                    "browseId":"UCPlaylistAuthor",
                    "browseEndpointContextSupportedConfigs":{
                        "browseEndpointContextMusicConfig":{"pageType":page_type}
                    }
                }}
            }]}})
        };
        let author = owner(&header("MUSIC_PAGE_TYPE_ARTIST")).unwrap();
        assert_eq!(author.name(), "Playlist author");
        assert_eq!(
            author.reference(),
            ObjectRef::native(PROVIDER, ObjectKind::Artist, "UCPlaylistAuthor".into()).as_ref()
        );
        for kind in [
            "MUSIC_PAGE_TYPE_USER_CHANNEL",
            "MUSIC_PAGE_TYPE_UNKNOWN",
            "",
        ] {
            assert_eq!(
                owner(&header(kind)),
                Some(PlaylistOwner::Named("Playlist author".into()))
            );
        }
    }

    #[test]
    fn author_facepile_links_only_a_single_verified_artist() {
        let mut header = json!({"facepile":{"avatarStackViewModel":{
            "avatars":[{"avatarViewModel":{}}],
            "text":{"content":"The Beatles"},
            "rendererContext":{"commandContext":{"onTap":{"innertubeCommand":{"browseEndpoint":{
                "browseId":"UCc4K7bAqpdBP8jh1j9XZAww",
                "browseEndpointContextSupportedConfigs":{
                    "browseEndpointContextMusicConfig":{"pageType":"MUSIC_PAGE_TYPE_ARTIST"}
                }
            }}}}}
        }}});
        let author = owner(&header).unwrap();
        assert_eq!(author.name(), "The Beatles");
        assert_eq!(author.reference().unwrap().id, "UCc4K7bAqpdBP8jh1j9XZAww");
        header["facepile"]["avatarStackViewModel"]["avatars"] = json!([{}, {}]);
        assert_eq!(
            owner(&header),
            Some(PlaylistOwner::Named("The Beatles".into()))
        );
    }

    #[test]
    fn preserves_complete_artist_runs_and_uses_display_for_an_unlinked_guest() {
        let artist = |name: &str, id: &str| {
            json!({
                "text": name,
                "navigationEndpoint": {"browseEndpoint": {"browseId": id}},
            })
        };
        let mut row = json!({
            "playlistItemData": {"videoId": "videoID1234"},
            "flexColumns": [
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"simpleText": "Song"}}},
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                    artist("Joey Valence & Brae", "UCBand"),
                    {"text": " и "},
                    artist("JPEGMAFIA", "UCGuest"),
                    {"text": " • "},
                    {"text": "184 тыс. просмотров"}
                ]}}},
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                    {"text": "Release", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPRErecord"}}}
                ]}}}
            ]
        });
        assert_eq!(track(&row, 0).artists, ["Joey Valence & Brae", "JPEGMAFIA"]);
        row["flexColumns"][1]["musicResponsiveListItemFlexColumnRenderer"]["text"]["runs"][2] =
            json!({"text": "JPEGMAFIA"});
        let track = track(&row, 0);
        assert!(track.artists.is_empty());
        assert!(track.artist.contains("JPEGMAFIA"));
        assert_eq!(track.artist_credits.len(), 2);
        assert_eq!(track.artist_credits[0].name, "Joey Valence & Brae");
        assert_eq!(
            track.artist_credits[0].reference.as_ref().unwrap().id,
            "UCBand"
        );
        assert_eq!(track.artist_credits[1].name, "JPEGMAFIA");
        assert!(track.artist_credits[1].reference.is_none());
        assert_eq!(track.album_ref.unwrap().id, "MPRErecord");
    }

    #[test]
    fn keeps_tracks_when_a_continuation_repeats_a_used_token() {
        for repeated in ["first", "last"] {
            let mut requests = Vec::new();
            let tracks = load_tracks(empty_page("first"), &AtomicBool::new(false), |token| {
                requests.push(token.to_owned());
                Ok(match token {
                    "first" => action_page("videoID1234", "last"),
                    "last" => action_page("videoID5678", repeated),
                    _ => panic!("unexpected continuation"),
                })
            })
            .unwrap();

            assert_eq!(requests, ["first", "last"]);
            assert_eq!(
                tracks
                    .iter()
                    .map(|track| track.id.as_str())
                    .collect::<Vec<_>>(),
                ["videoID1234", "videoID5678"]
            );
        }
    }

    #[test]
    fn emits_contiguous_rows_before_requesting_the_next_cursor() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());
        let tracks = load_track_pages(
            action_page("videoID1234", "next"),
            &AtomicBool::new(false),
            |token| {
                events.borrow_mut().push(token.to_owned());
                Ok(action_page("videoID5678", "next"))
            },
            |tracks, offset| {
                assert_eq!(offset, 0);
                assert!(tracks[0].media.is_none());
                events.borrow_mut().push(tracks[0].id.clone());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(events.into_inner(), ["videoID1234", "next"]);
        assert_eq!(tracks.len(), 2);
    }

    #[test]
    fn stops_at_the_first_empty_continuation_with_another_token() {
        let mut requests = Vec::new();
        let error = load_tracks(empty_page("first"), &AtomicBool::new(false), |token| {
            requests.push(token.to_owned());
            assert_eq!(
                token, "first",
                "empty continuation must not fetch the next token"
            );
            Ok(empty_page("different"))
        })
        .unwrap_err();
        assert_eq!(requests, ["first"]);
        assert_eq!(error.kind, ErrorKind::InvalidResponse);
    }

    #[test]
    fn accepts_a_header_only_first_page_and_an_empty_final_page() {
        let mut requests = Vec::new();
        let tracks = load_tracks(empty_page("first"), &AtomicBool::new(false), |token| {
            requests.push(token.to_owned());
            Ok(match token {
                "first" => json!({"musicPlaylistShelfContinuation": {
                    "contents": [{"musicResponsiveListItemRenderer": {
                        "playlistItemData": {"videoId":"videoID1234"}
                    }}],
                    "continuations": [{"nextContinuationData":{"continuation":"last"}}]
                }}),
                "last" => json!({"musicPlaylistShelfContinuation": {"contents": []}}),
                _ => panic!("unexpected continuation"),
            })
        })
        .unwrap();
        assert_eq!(requests, ["first", "last"]);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, "videoID1234");
    }

    fn empty_page(continuation: &str) -> Value {
        json!({"musicPlaylistShelfRenderer": {
            "contents": [],
            "continuations": [{"nextContinuationData":{"continuation":continuation}}]
        }})
    }

    fn action_page(video_id: &str, next: &str) -> Value {
        json!({"onResponseReceivedActions": [{"appendContinuationItemsAction": {
            "continuationItems": [
                {"musicResponsiveListItemRenderer": {"playlistItemData": {"videoId": video_id}}},
                {"continuationItemRenderer": {"continuationEndpoint": {"continuationCommand": {"token": next}}}}
            ]
        }}]})
    }
}
