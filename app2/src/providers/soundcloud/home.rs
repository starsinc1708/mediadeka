mod snapshot;

use std::{collections::HashSet, sync::atomic::AtomicBool, thread};

use serde_json::Value;

use super::{PROVIDER, feed, item};
use crate::providers::{
    HomeItem, ProviderError,
    transport::{HOME_LIMIT, check_cancelled, field, invalid_response},
};
pub use snapshot::SoundCloudShelf;
pub(crate) use snapshot::SoundCloudUpdate;

const MIXED_LIMIT: usize = 20;
const HISTORY: &str = "/me/play-history/tracks";
const CONTEXTS: &str = "/me/play-history/contexts";
const TRENDING: &str = "/charts?kind=trending&genre=soundcloud:genres:all-music";
// The public SoundCloud editorial profile at soundcloud.com/buzzing-playlists.
const BUZZING: &str = "/users/1356192489/playlists_without_albums";

pub(crate) fn load(
    profile_id: &str,
    cancelled: &AtomicBool,
    get: impl Fn(&str) -> Result<Value, ProviderError> + Sync,
) -> Result<SoundCloudUpdate, ProviderError> {
    let id = profile_id
        .parse::<u64>()
        .map_err(|_| invalid_response(PROVIDER))?;
    let get = |path: &str| {
        check_cancelled(cancelled)?;
        get(path)
    };
    let collection = |path: &str| {
        let query = if path.contains('?') { '&' } else { '?' };
        get(&format!(
            "{path}{query}limit={HOME_LIMIT}&linked_partitioning=1"
        ))
    };
    let (activity, discover, buzzing, [recent, history, likes, following]) =
        thread::scope(|scope| {
            let activity = scope.spawn(|| feed::load(cancelled, get));
            let discover = scope.spawn(|| {
                match get(&format!("/mixed-selections?limit={MIXED_LIMIT}")).and_then(mixed_shelves)
                {
                    Ok(shelves) if !shelves.is_empty() => Ok(shelves),
                    mixed => match collection(TRENDING).and_then(tracks) {
                        Ok(items) => Ok(vec![("Треки в тренде".to_owned(), items)]),
                        Err(error) => mixed.map_err(|_| error),
                    },
                }
            });
            let buzzing = scope.spawn(|| {
                collection(BUZZING).and_then(|value| {
                    collect_items(&value, |playlist| {
                        if playlist["user"]["permalink"] != "buzzing-playlists" {
                            return None;
                        }
                        item::card(playlist)
                    })
                })
            });
            let recent = scope.spawn(|| collection(CONTEXTS).and_then(recent_contexts));
            let history = scope.spawn(|| collection(HISTORY).and_then(tracks));
            let likes =
                scope.spawn(|| collection(&format!("/users/{id}/track_likes")).and_then(tracks));
            let following = scope.spawn(|| {
                collection(&format!("/users/{id}/followings"))
                    .and_then(|value| collect_items(&value, item::artist))
            });
            (
                activity.join().expect("SoundCloud activity worker"),
                discover.join().expect("SoundCloud discovery worker"),
                buzzing.join().expect("SoundCloud Buzzing worker"),
                [
                    recent.join().expect("SoundCloud recent contexts worker"),
                    history.join().expect("SoundCloud history worker"),
                    likes.join().expect("SoundCloud likes worker"),
                    following.join().expect("SoundCloud following worker"),
                ],
            )
        });
    check_cancelled(cancelled)?;

    use SoundCloudShelf::{Activity, Buzzing, Discover, Following, History, Likes, Recent};
    SoundCloudUpdate::new([
        (Discover, discover),
        Buzzing.with_items(buzzing),
        Activity.with_items(activity),
        Recent.with_items(recent),
        History.with_items(history),
        Likes.with_items(likes),
        Following.with_items(following),
    ])
}

fn mixed_shelves(value: Value) -> Result<Vec<(String, Vec<HomeItem>)>, ProviderError> {
    Ok(rows(&value)?
        .iter()
        .take(MIXED_LIMIT)
        .filter_map(|shelf| {
            let title = field(shelf, "title")?;
            let items = collect_items(&shelf["items"], item::card).ok()?;
            (!items.is_empty()).then_some((title, items))
        })
        .collect())
}

fn tracks(value: Value) -> Result<Vec<HomeItem>, ProviderError> {
    collect_items(&value, |row| item::track(&row["track"]))
}

fn recent_contexts(value: Value) -> Result<Vec<HomeItem>, ProviderError> {
    collect_items(&value, |row| {
        let key = match row["kind"].as_str()? {
            "system-playlist" => "system_playlist",
            other @ ("track" | "playlist" | "station" | "user") => other,
            _ => return None,
        };
        item::card(&row[key])
    })
}

fn rows(value: &Value) -> Result<&[Value], ProviderError> {
    value["collection"]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| invalid_response(PROVIDER))
}

fn collect_items(
    value: &Value,
    parse: impl Fn(&Value) -> Option<HomeItem>,
) -> Result<Vec<HomeItem>, ProviderError> {
    let mut seen = HashSet::new();
    Ok(rows(value)?
        .iter()
        .filter_map(parse)
        .filter(|item| seen.insert(item.id.clone()))
        .take(HOME_LIMIT)
        .collect())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::providers::{ErrorKind, HomeSectionPage, HomeSectionStyle};

    #[test]
    fn organizes_mixed_cards_and_library_without_duplicating_cards_in_a_shelf() {
        let mix = json!({
            "kind": "system-playlist", "urn": "soundcloud:system-playlists:your-mix:example",
            "title": "Your Mix 1", "short_description": "For you",
            "calculated_artwork_url": "https://i1.sndcdn.com/mix-t500x500.jpg?tracking=example",
            "permalink_url": "https://soundcloud.com/discover/sets/your-mix:example?tracking=example"
        });
        let artist = json!({
            "kind": "user", "id": 42, "username": "Artist", "followers_count": 53,
            "avatar_url": "https://i1.sndcdn.com/avatar.jpg", "permalink_url": "https://soundcloud.com/artist"
        });
        let track = json!({
            "kind": "track", "id": 42, "title": "Track", "user": artist,
            "permalink_url": "https://soundcloud.com/artist/track"
        });
        let home = load("123", &AtomicBool::new(false), |path| {
            Ok(match path.split('?').next().unwrap() {
                "/stream" => json!({"collection":[{"type":"track", "track":track}]}),
                "/mixed-selections" => json!({"collection":[
                    {"title":"For you", "items":{"collection":[mix, artist, mix, {"kind":"promotion"}]}},
                    {"title":"Empty", "items":{"collection":[]}}
                ]}),
                CONTEXTS => json!({"collection":[{"kind":"system-playlist", "system_playlist":mix}]}),
                HISTORY | "/users/123/track_likes" => json!({"collection":[{"track":track}]}),
                "/users/123/followings" => json!({"collection":[artist]}),
                BUZZING => json!({"collection":[]}),
                _ => unreachable!(),
            })
        }).unwrap().apply(None).0;
        let sections = home
            .sections
            .iter()
            .map(|shelf| (shelf.page, shelf.start, shelf.len));
        use HomeSectionPage::{Activity, Home as Discover, Library};
        assert_eq!(
            sections.collect::<Vec<_>>(),
            [
                (Discover, 0, 2),
                (Activity, 2, 1),
                (Library, 3, 1),
                (Library, 4, 1),
                (Library, 5, 1),
                (Library, 6, 1),
            ]
        );
        assert_eq!(
            home.items[0].artwork.as_deref(),
            Some("https://i1.sndcdn.com/mix-t500x500.jpg")
        );
        assert_eq!(
            home.items[0].url.as_deref(),
            Some("https://soundcloud.com/discover/sets/your-mix:example")
        );
        assert!(home.items[1].is_artist());
        assert_eq!(home.items[1].subtitle, "53 подписчика");
        assert_eq!(home.sections[5].style, HomeSectionStyle::Artists);
    }

    #[test]
    fn uses_available_pages_and_leaves_full_outages_to_the_account_cache() {
        let unavailable = || ProviderError::new(ErrorKind::Network, "offline");
        let cancelled = AtomicBool::new(false);
        let update = load("123", &cancelled, |path| {
            if path.starts_with(TRENDING) {
                Ok(json!({"collection":[{"track":{"title":"Trend", "id":1}}]}))
            } else {
                Err(unavailable())
            }
        })
        .unwrap();
        assert!(update.is_partial());
        let (home, issue) = update.apply(None);
        assert_eq!(issue, Some(unavailable()));
        assert_eq!(home.items[0].title, "Trend");
        assert_eq!(home.sections[0].page, HomeSectionPage::Home);
        assert_eq!(
            load("123", &cancelled, |_| Err(unavailable())).err(),
            Some(unavailable())
        );

        let denied = ProviderError::new(ErrorKind::Rejected, "HTTP 403");
        let result = load("123", &cancelled, |path| {
            Err(if path.starts_with(HISTORY) {
                denied.clone()
            } else {
                unavailable()
            })
        });
        assert_eq!(result.err(), Some(denied));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires an isolated profile with a live SoundCloud account"]
    fn live_soundcloud_home_sections() {
        use crate::{providers, storage::Store};

        let path = std::env::var_os("MEDIADEKA_SOUNDCLOUD_TEST_PROFILE")
            .expect("isolated test profile path");
        let store = Store::open(path).unwrap();
        let account = store.preferred_account(PROVIDER).unwrap().unwrap();
        let credentials = store.credentials(&account).unwrap().unwrap();
        let cancelled = AtomicBool::new(false);
        let session =
            providers::restore(PROVIDER, &account.profile, credentials, &cancelled).unwrap();
        let home = providers::home(&session, &cancelled).unwrap();
        if let Some(mix_artwork) = home
            .items
            .iter()
            .filter_map(|item| item.artwork.as_deref())
            .find(|url| url.starts_with("https://al.sndcdn.com/"))
        {
            assert!(crate::providers::is_public_artwork_url(mix_artwork));
            let image = crate::providers::transport::agent()
                .get(mix_artwork)
                .call()
                .unwrap();
            assert!(image.status().is_success());
            assert_eq!(image.headers().get("Content-Type").unwrap(), "image/jpeg");
            println!("SoundCloud mix artwork downloaded successfully");
        }
        for page in [
            HomeSectionPage::Home,
            HomeSectionPage::Activity,
            HomeSectionPage::Library,
        ] {
            let shelves: Vec<_> = home
                .sections
                .iter()
                .filter(|shelf| shelf.page == page)
                .collect();
            let items = shelves
                .iter()
                .flat_map(|shelf| &home.items[shelf.start..shelf.start + shelf.len]);
            let (mut count, mut covers, mut links) = (0, 0, 0);
            for item in items {
                count += 1;
                covers += usize::from(item.artwork.is_some());
                links += usize::from(item.url.is_some());
            }
            println!(
                "{page:?}: {} sections, {count} cards, {covers} covers, {links} links",
                shelves.len()
            );
            assert!(
                count > 0 && covers > 0 && links > 0,
                "{page:?} response is incomplete"
            );
        }
    }
}
