use url::Url;

use crate::{
    music::ObjectKind::{self, Album, Artist, Collection, Playlist, Station, Track},
    providers::{
        soundcloud_artist_station_from_slug, soundcloud_station_url, yandex as yandex_provider,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PublicLink {
    pub(crate) platform: String,
    pub(crate) kind: ObjectKind,
    pub(crate) id: String,
    pub(crate) url: String,
}

impl PublicLink {
    pub(super) fn same_address(&self, other: &Self) -> bool {
        self.platform == other.platform && self.kind == other.kind && self.id == other.id
    }
}

/// Recognize direct public routes and remove referral, context, and private share parameters.
pub(crate) fn public_link(value: &str) -> Option<PublicLink> {
    if let Some(uri) = value.trim().strip_prefix("spotify:") {
        let (kind, id) = uri.split_once(':')?;
        return spotify_object(kind, id);
    }
    from_https_url(&parse_https(value, None)?).ok()
}

pub(super) fn parse_https(value: &str, base: Option<&Url>) -> Option<Url> {
    let value = value.trim().trim_matches(|ch| ch <= ' ');
    let path = value.split(['?', '#']).next()?;
    if path.contains(['\\', '\t', '\r', '\n']) {
        return None;
    }
    let authority = if let Some((scheme, rest)) = path.split_once(':')
        && scheme.eq_ignore_ascii_case("https")
    {
        Some(rest.strip_prefix("//")?)
    } else {
        path.strip_prefix("//")
    };
    // URL parsing discards an explicit default port and empty user information.
    if authority.is_some_and(|rest| {
        let host = rest.split('/').next().unwrap_or_default();
        host.is_empty() || host.contains([':', '@'])
    }) {
        return None;
    }
    let url = Url::options().base_url(base).parse(value).ok()?;
    (url.scheme() == "https"
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

pub(super) enum AddressError {
    UnknownHost,
    NoObject,
}

pub(super) fn from_https_url(url: &Url) -> Result<PublicLink, AddressError> {
    use AddressError::{NoObject, UnknownHost};

    let segments: Vec<_> = url.path_segments().ok_or(NoObject)?.collect();
    let path = segments.strip_suffix(&[""]).unwrap_or(&segments);
    match url.host_str().ok_or(UnknownHost)? {
        "open.spotify.com" => spotify(path),
        "music.apple.com" | "itunes.apple.com" => apple(url, path),
        "geo.music.apple.com" => matches!(without_locale(path, country), ["song", ..])
            .then(|| apple(url, path))
            .flatten(),
        "music.youtube.com" | "www.youtube.com" | "youtube.com" | "youtu.be" => youtube(url, path),
        "m.youtube.com" => matches!(path, ["watch" | "playlist"])
            .then(|| youtube(url, path))
            .flatten(),
        "music.yandex.ru" | "music.yandex.com" | "music.yandex.kz" | "music.yandex.by"
        | "music.yandex.uz" => yandex(path),
        "soundcloud.com" | "www.soundcloud.com" | "m.soundcloud.com" => {
            if query_value(url, "secret_token").is_some() {
                return Err(NoObject);
            }
            soundcloud(path)
        }
        "deezer.com" | "www.deezer.com" => numeric_route(
            "deezer",
            "https://www.deezer.com",
            without_locale(path, country),
        ),
        "tidal.com" | "listen.tidal.com" | "www.tidal.com" => tidal(path),
        "open.qobuz.com" => numeric_route("qobuz", "https://open.qobuz.com", path),
        "music.amazon.com" | "music.amazon.co.uk" | "music.amazon.de" | "music.amazon.fr"
        | "music.amazon.co.jp" | "music.amazon.it" | "music.amazon.es" => amazon(url, path),
        "zvuk.com" | "www.zvuk.com" => {
            let [kind, id] = path else {
                return Err(NoObject);
            };
            let kind = match *kind {
                "release" => Album,
                "track" => Track,
                "artist" => Artist,
                _ => return Err(NoObject),
            };
            digits(id).then(|| {
                entry(
                    "zvuk",
                    kind,
                    id,
                    format!("https://zvuk.com/{}", path.join("/")),
                )
            })
        }
        _ => return Err(UnknownHost),
    }
    .ok_or(NoObject)
}

fn spotify(path: &[&str]) -> Option<PublicLink> {
    let [kind, id] = without_locale(path, |part| part.strip_prefix("intl-").is_some_and(country))
    else {
        return None;
    };
    match *kind {
        "genre" | "page" if slug(id) => Some(spotify_entry(Collection, id)),
        kind => spotify_object(kind, id),
    }
}

fn spotify_object(kind: &str, id: &str) -> Option<PublicLink> {
    const ID_LENGTH: usize = 22;
    let kind = object_kind(kind)?;
    (id.len() == ID_LENGTH && alphanumeric(id)).then(|| spotify_entry(kind, id))
}

fn spotify_entry(kind: ObjectKind, id: &str) -> PublicLink {
    entry(
        "spotify",
        kind,
        id,
        format!("https://open.spotify.com/{}/{id}", kind.path()),
    )
}

fn apple(url: &Url, path: &[&str]) -> Option<PublicLink> {
    let [route, remainder @ ..] = without_locale(path, country) else {
        return None;
    };
    let storefront = path
        .first()
        .copied()
        .filter(|region| country(region))
        .unwrap_or("us");
    let id = match remainder {
        [id] | [_, id] => *id,
        _ => return None,
    };
    let base = format!("https://music.apple.com/{storefront}/{route}/{id}");
    if *route == "playlist" {
        return id
            .strip_prefix("pl.")
            .filter(|id| slug(id))
            .map(|_| entry("apple-music", Playlist, id, base));
    }
    if !digits(id) {
        return None;
    }
    if *route == "album"
        && let Some(track) = query_value(url, "i")
    {
        return digits(&track)
            .then(|| entry("apple-music", Track, &track, format!("{base}?i={track}")));
    }
    let kind = match *route {
        "song" => Track,
        "album" => Album,
        "artist" => Artist,
        _ => return None,
    };
    Some(entry("apple-music", kind, id, base))
}

fn youtube(url: &Url, path: &[&str]) -> Option<PublicLink> {
    let (kind, id) = match (url.host_str()?, path) {
        ("youtu.be", [id]) if video_id(id) => (Track, (*id).into()),
        ("youtu.be", _) => return None,
        (_, ["shorts", id]) if video_id(id) => (Track, (*id).into()),
        (_, ["watch"]) => (Track, query_value(url, "v").filter(|id| video_id(id))?),
        (_, ["playlist"]) => {
            let id = query_value(url, "list").filter(|id| slug(id))?;
            if id.starts_with("OLAK5uy_") && id.len() > "OLAK5uy_".len() {
                (Album, id)
            } else {
                (Playlist, id)
            }
        }
        (_, ["browse", id]) if id.starts_with("MPRE") && id.len() > 4 && slug(id) => {
            (Album, (*id).into())
        }
        (_, ["browse" | "channel", id]) if id.starts_with("UC") && id.len() == 24 && slug(id) => {
            (Artist, (*id).into())
        }
        (_, ["browse", id]) if id.starts_with("VL") && id.len() > 2 && slug(id) => {
            (Playlist, id[2..].into())
        }
        _ => return None,
    };
    let route = match kind {
        Track => format!("watch?v={id}"),
        Artist => format!("channel/{id}"),
        Album if id.starts_with("MPRE") => format!("browse/{id}"),
        Album | Playlist => format!("playlist?list={id}"),
        Collection | Station => return None,
    };
    Some(entry(
        "youtube-music",
        kind,
        &id,
        format!("https://music.youtube.com/{route}"),
    ))
}

fn yandex(path: &[&str]) -> Option<PublicLink> {
    let path = match path {
        ["artist", _, "tracks" | "albums"] => &path[..2],
        _ => path,
    };
    let (kind, id) = match path {
        ["album", album, "track", track] if digits(album) && digits(track) => {
            (Track, format!("{track}:{album}"))
        }
        ["users", user, "playlists", id] => (Playlist, format!("{user}:{id}")),
        ["playlists", id] if !id.contains(':') => (Playlist, (*id).into()),
        ["tag", slug] => (Collection, format!("tag:{slug}")),
        [kind @ ("album" | "track" | "artist"), id] if digits(id) => {
            (object_kind(kind)?, (*id).into())
        }
        _ => return None,
    };
    let url = match kind {
        Playlist => yandex_provider::playlist_url(&id)?,
        Collection => yandex_provider::tag_url(&id)?,
        _ => format!("https://music.yandex.ru/{}", path.join("/")),
    };
    Some(entry("yandex-music", kind, &id, url))
}

fn soundcloud(path: &[&str]) -> Option<PublicLink> {
    if let ["discover", "sets", station] = path {
        let id =
            soundcloud_artist_station_from_slug(&station.replace("%3A", ":").replace("%3a", ":"))?;
        let url = soundcloud_station_url(&id)?;
        return Some(entry("soundcloud", Station, &id, url));
    }
    let owner = *path.first()?;
    if !slug(owner)
        || matches!(
            owner,
            "discover"
                | "stream"
                | "search"
                | "you"
                | "charts"
                | "upload"
                | "settings"
                | "terms-of-use"
                | "privacy-policy"
                | "pages"
                | "stations"
        )
    {
        return None;
    }
    let path = match path {
        [
            _,
            "tracks" | "albums" | "sets" | "reposts" | "likes" | "popular-tracks",
        ] => &path[..1],
        _ => path,
    };
    let kind = match path {
        [_] => Artist,
        [_, "sets", list] if slug(list) => Playlist,
        [_, id] if slug(id) && !matches!(*id, "followers" | "following") => Track,
        _ => return None,
    };
    let id = path.join("/");
    Some(entry(
        "soundcloud",
        kind,
        &id,
        format!("https://soundcloud.com/{id}"),
    ))
}

fn tidal(path: &[&str]) -> Option<PublicLink> {
    let path = path.strip_prefix(&["browse"]).unwrap_or(path);
    let ([kind, id] | [kind, _, id]) = path else {
        return None;
    };
    if *kind == "playlist" {
        return None;
    }
    numeric_route("tidal", "https://tidal.com/browse", &[*kind, *id])
}

fn amazon(url: &Url, path: &[&str]) -> Option<PublicLink> {
    let [kind, id] = path else { return None };
    if !asin(id) {
        return None;
    }
    let host = url.host_str()?;
    let canonical = format!("https://{host}/{kind}/{id}");
    let (kind, id, canonical) = match *kind {
        "tracks" => (Track, (*id).into(), canonical),
        "albums" => match query_value(url, "trackAsin") {
            Some(track) if asin(&track) => (
                Track,
                track.clone(),
                format!("{canonical}?trackAsin={track}"),
            ),
            Some(_) => return None,
            None => (Album, (*id).into(), canonical),
        },
        "artists" => (Artist, (*id).into(), canonical),
        _ => return None,
    };
    Some(entry("amazon-music", kind, &id, canonical))
}

fn numeric_route(platform: &str, origin: &str, path: &[&str]) -> Option<PublicLink> {
    let [kind, id] = path else { return None };
    Some(entry(
        platform,
        object_kind(kind)?,
        digits(id).then_some(id)?,
        format!("{origin}/{kind}/{id}"),
    ))
}

fn entry(platform: &str, kind: ObjectKind, id: &str, url: String) -> PublicLink {
    PublicLink {
        platform: platform.into(),
        kind,
        id: id.into(),
        url,
    }
}

fn object_kind(kind: &str) -> Option<ObjectKind> {
    match kind {
        "track" => Some(Track),
        "album" => Some(Album),
        "artist" => Some(Artist),
        "playlist" => Some(Playlist),
        _ => None,
    }
}

fn query_value(url: &Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find_map(|(name, value)| (name == key).then(|| value.into_owned()))
}

fn without_locale<'a>(path: &'a [&'a str], is_locale: impl FnOnce(&str) -> bool) -> &'a [&'a str] {
    match path {
        [first, rest @ ..] if is_locale(first) => rest,
        _ => path,
    }
}

fn country(value: &str) -> bool {
    value.len() == 2 && value.bytes().all(|byte| byte.is_ascii_alphabetic())
}

fn digits(value: &str) -> bool {
    !value.is_empty() && value.len() <= 24 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn alphanumeric(value: &str) -> bool {
    value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn video_id(value: &str) -> bool {
    value.len() == 11 && slug(value)
}

fn asin(value: &str) -> bool {
    value.len() == 10 && alphanumeric(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::SearchInput;

    const ACCEPTED: &[(&str, &str, ObjectKind, &str, &str)] = &[
        (
            "https://music.apple.com/ru/album/foo/1234?i=4321&uo=4",
            "apple-music",
            Track,
            "4321",
            "https://music.apple.com/ru/album/1234?i=4321",
        ),
        (
            "https://geo.music.apple.com/us/song/617154366?at=example",
            "apple-music",
            Track,
            "617154366",
            "https://music.apple.com/us/song/617154366",
        ),
        (
            "https://music.youtube.com/watch?v=GTv4F8bAB30&list=PL123",
            "youtube-music",
            Track,
            "GTv4F8bAB30",
            "https://music.youtube.com/watch?v=GTv4F8bAB30",
        ),
        (
            "https://music.yandex.ru/album/43177750/track/153919042?utm_source=test",
            "yandex-music",
            Track,
            "153919042:43177750",
            "https://music.yandex.ru/album/43177750/track/153919042",
        ),
        (
            "https://open.spotify.com/intl-de/track/1E96rC1JYgZoPPw8kvxd1I?si=private",
            "spotify",
            Track,
            "1E96rC1JYgZoPPw8kvxd1I",
            "https://open.spotify.com/track/1E96rC1JYgZoPPw8kvxd1I",
        ),
        (
            "https://music.youtube.com/playlist?list=OLAK5uy_6",
            "youtube-music",
            Album,
            "OLAK5uy_6",
            "https://music.youtube.com/playlist?list=OLAK5uy_6",
        ),
        (
            "https://music.youtube.com/browse/MPREb_AB12",
            "youtube-music",
            Album,
            "MPREb_AB12",
            "https://music.youtube.com/browse/MPREb_AB12",
        ),
        (
            "https://music.youtube.com/playlist?list=PLZ60rAdTLVNuCAWD8W3dZTLBXbSPda-DX",
            "youtube-music",
            Playlist,
            "PLZ60rAdTLVNuCAWD8W3dZTLBXbSPda-DX",
            "https://music.youtube.com/playlist?list=PLZ60rAdTLVNuCAWD8W3dZTLBXbSPda-DX",
        ),
        (
            "spotify:track:1E96rC1JYgZoPPw8kvxd1I",
            "spotify",
            Track,
            "1E96rC1JYgZoPPw8kvxd1I",
            "https://open.spotify.com/track/1E96rC1JYgZoPPw8kvxd1I",
        ),
        (
            "spotify:album:4m2880jivSbbyEGAKfITCa",
            "spotify",
            Album,
            "4m2880jivSbbyEGAKfITCa",
            "https://open.spotify.com/album/4m2880jivSbbyEGAKfITCa",
        ),
        (
            "spotify:artist:4tZwfgrHOc3mvqYlEYSvVi",
            "spotify",
            Artist,
            "4tZwfgrHOc3mvqYlEYSvVi",
            "https://open.spotify.com/artist/4tZwfgrHOc3mvqYlEYSvVi",
        ),
        (
            "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M",
            "spotify",
            Playlist,
            "37i9dQZF1DXcBWIGoYBM5M",
            "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M",
        ),
        (
            "https://music.yandex.ru/artist/36800/tracks",
            "yandex-music",
            Artist,
            "36800",
            "https://music.yandex.ru/artist/36800",
        ),
        (
            "https://music.yandex.ru/artist/36800/albums",
            "yandex-music",
            Artist,
            "36800",
            "https://music.yandex.ru/artist/36800",
        ),
        (
            "https://music.yandex.uz/album/43177750/track/153919042",
            "yandex-music",
            Track,
            "153919042:43177750",
            "https://music.yandex.ru/album/43177750/track/153919042",
        ),
        (
            "https://m.youtube.com/watch?v=GTv4F8bAB30",
            "youtube-music",
            Track,
            "GTv4F8bAB30",
            "https://music.youtube.com/watch?v=GTv4F8bAB30",
        ),
        (
            "https://m.youtube.com/playlist?list=PLZ60rAdTLVNuCAWD8W3dZTLBXbSPda-DX",
            "youtube-music",
            Playlist,
            "PLZ60rAdTLVNuCAWD8W3dZTLBXbSPda-DX",
            "https://music.youtube.com/playlist?list=PLZ60rAdTLVNuCAWD8W3dZTLBXbSPda-DX",
        ),
        (
            "https://m.soundcloud.com/daftpunkofficialmusic",
            "soundcloud",
            Artist,
            "daftpunkofficialmusic",
            "https://soundcloud.com/daftpunkofficialmusic",
        ),
        (
            "https://soundcloud.com/discover/sets/artist-stations:12345?utm_source=clipboard",
            "soundcloud",
            Station,
            "soundcloud:system-playlists:artist-stations:12345",
            "https://soundcloud.com/discover/sets/artist-stations:12345",
        ),
        (
            "https://soundcloud.com/discover/sets/artist-stations%3A12345%3a67890",
            "soundcloud",
            Station,
            "soundcloud:system-playlists:artist-stations:12345:67890",
            "https://soundcloud.com/discover/sets/artist-stations:12345:67890",
        ),
        (
            "https://soundcloud.com/daftpunkofficialmusic/tracks",
            "soundcloud",
            Artist,
            "daftpunkofficialmusic",
            "https://soundcloud.com/daftpunkofficialmusic",
        ),
        (
            "https://soundcloud.com/daftpunkofficialmusic/albums",
            "soundcloud",
            Artist,
            "daftpunkofficialmusic",
            "https://soundcloud.com/daftpunkofficialmusic",
        ),
        (
            "https://soundcloud.com/daftpunkofficialmusic/sets",
            "soundcloud",
            Artist,
            "daftpunkofficialmusic",
            "https://soundcloud.com/daftpunkofficialmusic",
        ),
        (
            "https://soundcloud.com/daftpunkofficialmusic/reposts",
            "soundcloud",
            Artist,
            "daftpunkofficialmusic",
            "https://soundcloud.com/daftpunkofficialmusic",
        ),
        (
            "https://soundcloud.com/daftpunkofficialmusic/likes",
            "soundcloud",
            Artist,
            "daftpunkofficialmusic",
            "https://soundcloud.com/daftpunkofficialmusic",
        ),
        (
            "https://soundcloud.com/daftpunkofficialmusic/popular-tracks",
            "soundcloud",
            Artist,
            "daftpunkofficialmusic",
            "https://soundcloud.com/daftpunkofficialmusic",
        ),
        (
            "https://music.yandex.ru/users/yamusic-podcast/playlists/1019",
            "yandex-music",
            Playlist,
            "yamusic-podcast:1019",
            "https://music.yandex.ru/users/yamusic-podcast/playlists/1019",
        ),
        (
            "https://music.yandex.uz/playlists/d54b98ef-0250-3e6d-aa13-4c0fb7bb0c43?utm_source=share",
            "yandex-music",
            Playlist,
            "d54b98ef-0250-3e6d-aa13-4c0fb7bb0c43",
            "https://music.yandex.ru/playlists/d54b98ef-0250-3e6d-aa13-4c0fb7bb0c43",
        ),
        (
            "https://music.yandex.ru/tag/autumn/?utm_source=share",
            "yandex-music",
            Collection,
            "tag:autumn",
            "https://music.yandex.ru/tag/autumn",
        ),
    ];

    const REJECTED: &[&str] = &[
        "https://music.youtube.com/search?q=GATASKI",
        "https://music.youtube.com/playlist",
        "https://music.amazon.com/search/GATASKI",
        "https://music.apple.com/us/album/test/6795125260?i=invalid",
        "https://www.deezer.com.example.com/track/4199566742",
        "https://open.spotify.com@evil.invalid/track/1E96rC1JYgZoPPw8kvxd1I",
        "https://soundcloud.com/artist/recording/private-key",
        "https://soundcloud.com/artist/recording?secret_token=s-abc",
        "https://soundcloud.com/search",
        "https://soundcloud.com/discover/sets/artist-stations:123:456:789",
        "https://soundcloud.com/discover/sets/artist-stations:123?secret_token=s-abc",
        "http://open.spotify.com/track/1E96rC1JYgZoPPw8kvxd1I",
        "https://open.spotify.com:444/track/1E96rC1JYgZoPPw8kvxd1I",
        "https://open.spotify.com:443/track/1E96rC1JYgZoPPw8kvxd1I",
        "https://@open.spotify.com/track/1E96rC1JYgZoPPw8kvxd1I",
        "https://user:password@open.spotify.com/track/1E96rC1JYgZoPPw8kvxd1I",
        "spotify:track:1E96rC1JYgZoPPw8kvxd1",
        "spotify:track:1E96rC1JYgZoPPw8kvxd1!",
        "spotify:track:1E96rC1JYgZoPPw8kvxd1I?si=referral",
        "spotify:user:1E96rC1JYgZoPPw8kvxd1I",
        "spotify:local:1E96rC1JYgZoPPw8kvxd1I",
        "spotify:episode:1E96rC1JYgZoPPw8kvxd1I",
        "spotify:show:1E96rC1JYgZoPPw8kvxd1I",
        "spotify:search:1E96rC1JYgZoPPw8kvxd1I",
        "spotify:genre:1E96rC1JYgZoPPw8kvxd1I",
        "spotify:page:1E96rC1JYgZoPPw8kvxd1I",
        "https://music.yandex.uz/artist/name/albums",
        "https://music.yandex.ru/artist/36800/videos",
        "https://m.youtube.com/search?q=GATASKI",
        "https://m.youtube.com/shorts/GTv4F8bAB30",
        "https://m.soundcloud.com.example.com/daftpunkofficialmusic",
        "https://m.soundcloud.com/artist/sets/playlist/private-key",
        "https://m.soundcloud.com/artist/likes?secret_token=s-abc",
        "https://soundcloud.com/search/tracks",
        "https://soundcloud.com/daftpunkofficialmusic/followers",
        "https://soundcloud.com/daftpunkofficialmusic/following",
        "https://music.yandex.ru/playlists",
        "https://music.yandex.ru/playlists/yamusic-podcast:1019",
        "https://music.yandex.ru/playlists/d54b98ef-0250-3e6d-aa13-4c0fb7bb0c43/extra",
        "https://music.yandex.ru/playlists/uuid",
        "https://music.yandex.ru/users/yamusic-podcast/playlists/",
        "https://music.yandex.ru/tag/",
        "https://music.yandex.ru/tag/autumn/extra",
        "https://music.yandex.ru/tag/autumn%2Fextra",
    ];

    #[test]
    fn accepts_public_addresses() {
        for &(input, platform, kind, id, canonical) in ACCEPTED {
            assert_eq!(
                public_link(input),
                Some(PublicLink {
                    platform: platform.into(),
                    kind,
                    id: id.into(),
                    url: canonical.into(),
                }),
                "{input}"
            );
        }
    }

    #[test]
    fn catalog_search_recognizes_supported_spotify_uris_as_links() {
        for &(input, _, _, _, _) in ACCEPTED.iter().filter(|row| row.0.starts_with("spotify:")) {
            assert_eq!(
                SearchInput::new(Track, input),
                Some(SearchInput::Url(input.into())),
                "{input}"
            );
        }
        for &input in REJECTED
            .iter()
            .filter(|input| input.starts_with("spotify:"))
        {
            assert!(
                matches!(
                    SearchInput::new(Track, input),
                    Some(SearchInput::Text { .. })
                ),
                "{input}"
            );
        }
    }

    #[test]
    fn rejects_search_and_ambiguous_or_untrusted_addresses() {
        for &input in REJECTED {
            assert!(public_link(input).is_none(), "{input}");
        }
    }
}
