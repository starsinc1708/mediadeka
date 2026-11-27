use serde_json::Value;
use url::Url;

use super::PROVIDER;
use crate::music::ObjectKind;
use crate::playlist::ArtistCredit;
use crate::providers::{
    HomeItem,
    transport::{artwork_url, field},
};

pub(super) fn track(value: &Value) -> Option<HomeItem> {
    if value["kind"].as_str().is_some_and(|kind| kind != "track") {
        return None;
    }
    item(value, Kind::Track)
}

pub(super) fn artist(value: &Value) -> Option<HomeItem> {
    item(value, Kind::User)
}

pub(super) fn artist_credit(value: &Value) -> Option<ArtistCredit> {
    let owner = &value["user"];
    let username = field(owner, "username");
    let name = field(&value["publisher_metadata"], "artist").or_else(|| username.clone())?;
    // The uploader is an identity source only when it is also the displayed credit.
    let id = if username.as_deref() == Some(name.as_str()) {
        owner["id"]
            .as_u64()
            .filter(|id| *id != 0)
            .map(|id| id.to_string())
    } else {
        None
    };
    Some(ArtistCredit::from_catalog(name, PROVIDER, id))
}

pub(super) fn is_album(value: &Value) -> bool {
    value["is_album"] == true || value["set_type"] == "album"
}

pub(super) fn permalink(id: &str, kind: ObjectKind) -> Option<Url> {
    let parts = id.split('/').collect::<Vec<_>>();
    let shape = match kind {
        ObjectKind::Artist => matches!(parts.as_slice(), [_]),
        ObjectKind::Track => matches!(parts.as_slice(), [_, _]),
        ObjectKind::Album | ObjectKind::Playlist => matches!(parts.as_slice(), [_, "sets", _]),
        ObjectKind::Collection | ObjectKind::Station => false,
    };
    if !shape
        || id.len() > 300
        || parts.iter().any(|part| {
            part.is_empty()
                || matches!(*part, "." | "..")
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        })
    {
        return None;
    }
    Url::parse(&format!("https://soundcloud.com/{id}")).ok()
}

pub(super) fn card(value: &Value) -> Option<HomeItem> {
    let kind = match value["kind"].as_str()? {
        "track" => Kind::Track,
        "playlist" => Kind::Playlist,
        "system-playlist" | "station" => Kind::SystemPlaylist,
        "user" => Kind::User,
        _ => return None,
    };
    let mut card = item(value, kind)?;
    if value["kind"] == "station" {
        card.id.insert_str(0, "station:");
    }
    Some(card)
}

#[derive(Clone, Copy)]
enum Kind {
    Track,
    Playlist,
    SystemPlaylist,
    User,
}

fn item(value: &Value, kind: Kind) -> Option<HomeItem> {
    if value["promoted"].as_bool() == Some(true) {
        return None;
    }
    let owner = &value["user"];
    let (id, title, subtitle) = match kind {
        Kind::Track => (
            value["id"].as_u64()?.to_string(),
            field(value, "title")?,
            field(owner, "username"),
        ),
        Kind::Playlist => (
            format!(
                "{}:{}",
                if is_album(value) { "album" } else { "playlist" },
                value["id"].as_u64()?
            ),
            field(value, "title")?,
            field(owner, "username"),
        ),
        Kind::SystemPlaylist => (
            field(value, "urn")?,
            field(value, "title")?,
            field(value, "short_description").or_else(|| field(value, "description")),
        ),
        Kind::User => (
            format!("artist:{}", value["id"].as_u64()?),
            field(value, "username")?,
            value["followers_count"].as_u64().map(followers),
        ),
    };
    let (url, artwork) = if value["sharing"] == "private" {
        (None, None)
    } else {
        let url = value["permalink_url"]
            .as_str()
            .and_then(|url| public_url(url, kind));
        let artwork = [
            &value["artwork_url"],
            &value["calculated_artwork_url"],
            &value["avatar_url"],
            &owner["avatar_url"],
        ]
        .into_iter()
        .filter_map(Value::as_str)
        .find_map(|value| artwork_url(PROVIDER, value));
        (url, artwork)
    };
    Some(HomeItem {
        id,
        title,
        subtitle: subtitle.unwrap_or_default(),
        url,
        artwork,
    })
}

fn followers(count: u64) -> String {
    let unit = match (count % 100, count % 10) {
        (11..=14, _) => "подписчиков",
        (_, 1) => "подписчик",
        (_, 2..=4) => "подписчика",
        _ => "подписчиков",
    };
    format!("{count} {unit}")
}

fn public_url(value: &str, kind: Kind) -> Option<String> {
    let mut url = Url::parse(value).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("soundcloud.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    // Sharing links can contain secrets in extra path segments and query parameters.
    let segments: Vec<_> = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect();
    let supported = match kind {
        Kind::Track => matches!(&segments[..], [_, _]),
        Kind::Playlist => matches!(&segments[..], [_, "sets", _]),
        Kind::SystemPlaylist => matches!(&segments[..], ["discover", "sets", _]),
        Kind::User => matches!(&segments[..], [_]),
    };
    if !supported {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    Some(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soundcloud_links_credited_uploaders_without_substituting_other_artists() {
        let mut value = serde_json::json!({"user": {"id": 12, "username": "Uploader"}});
        let credit = artist_credit(&value).unwrap();
        assert_eq!(credit.name, "Uploader");
        assert_eq!(credit.reference.unwrap().id, "12");

        value["publisher_metadata"] = serde_json::json!({"artist": "Lead & Band"});
        let credit = artist_credit(&value).unwrap();
        assert_eq!(credit.name, "Lead & Band");
        assert!(credit.reference.is_none());
    }
}
