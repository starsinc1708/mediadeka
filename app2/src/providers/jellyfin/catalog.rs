use std::sync::atomic::AtomicBool;

use serde_json::Value;
use ureq::Agent;
use url::Url;

use crate::{
    music::{MusicPage, ObjectKind, ObjectRef},
    providers::{Credentials, ErrorKind, HomeItem, ProviderError, catalog, transport::*},
};

use super::{PROVIDER, at, auth, me, playlist, server, valid_item_id};

const FIELDS: &str = "ProviderIds";
const ALBUM_TRACK_SORT: &str = "ParentIndexNumber,IndexNumber,SortName";

pub(in crate::providers) fn resolve_link(
    credentials: &Credentials,
    web_root: &Url,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<ObjectRef, ProviderError> {
    check_cancelled(cancelled)?;
    if !valid_item_id(id) {
        return Err(catalog::invalid_id(PROVIDER));
    }
    let client = Client::open(credentials, Some(web_root))?;
    check_cancelled(cancelled)?;
    let row = client.request(&format!("Items/{id}"), &[])?;
    check_cancelled(cancelled)?;
    linked_reference(&row, id).ok_or_else(|| {
        ProviderError::new(
            ErrorKind::Unavailable,
            "Jellyfin не вернул музыкальный объект по этой ссылке.",
        )
    })
}

pub(in crate::providers) fn read(
    reference: &ObjectRef,
    credentials: &Credentials,
    cancelled: &AtomicBool,
) -> Result<MusicPage, ProviderError> {
    let id = reference.id.as_str();
    if !valid_item_id(id) || reference.kind == ObjectKind::Collection {
        return Err(catalog::invalid_id(PROVIDER));
    }
    let catalog = Client::open(credentials, None)?;
    check_cancelled(cancelled)?;
    let row = catalog.request(&format!("Items/{id}"), &[])?;
    let mut page = from_entry(&row, reference.kind)
        .filter(|page| same_item_id(&page.reference.id, id))
        .ok_or_else(|| invalid_response(PROVIDER))?;
    let id = page.reference.id.as_str();
    check_cancelled(cancelled)?;
    match reference.kind {
        ObjectKind::Album => {
            let data = catalog.request(
                "Items",
                &[
                    ("ParentId", id),
                    ("IncludeItemTypes", "Audio"),
                    ("SortBy", ALBUM_TRACK_SORT),
                ],
            )?;
            page.tracks = rows(&data)?
                .iter()
                .enumerate()
                .map(|(position, row)| playlist::track(row, position))
                .collect();
            page.fill_missing_track_albums();
        }
        ObjectKind::Artist => {
            let data = catalog.request(
                "Items",
                &[
                    ("AlbumArtistIds", id),
                    ("IncludeItemTypes", "MusicAlbum"),
                    ("Recursive", "true"),
                    ("SortBy", "ProductionYear,SortName"),
                    ("SortOrder", "Descending"),
                ],
            )?;
            page.releases = rows(&data)?
                .iter()
                .filter_map(|row| from_entry(row, ObjectKind::Album))
                .map(|album| HomeItem {
                    id: format!("album:{}", album.reference.id),
                    title: album.title,
                    subtitle: album.artists.join(", "),
                    url: None,
                    artwork: None,
                })
                .collect();
        }
        _ => {}
    }
    Ok(page)
}

pub(in crate::providers) fn search(
    credentials: &Credentials,
    kind: ObjectKind,
    query: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<MusicPage>, ProviderError> {
    if kind == ObjectKind::Collection {
        return Err(catalog::invalid_id(PROVIDER));
    }
    let client = Client::open(credentials, None)?;
    check_cancelled(cancelled)?;
    let data = client.request(
        "Items",
        &[
            ("IncludeItemTypes", item_type(kind)),
            ("Recursive", "true"),
            ("SearchTerm", query),
            ("Limit", &catalog::SEARCH_LIMIT.to_string()),
        ],
    )?;
    Ok(rows(&data)?
        .iter()
        .filter_map(|row| from_entry(row, kind))
        .collect())
}

struct Client {
    agent: Agent,
    address: Url,
    header: String,
    user: String,
}

impl Client {
    fn open(
        credentials: &Credentials,
        expected_web_root: Option<&Url>,
    ) -> Result<Self, ProviderError> {
        let Credentials::Jellyfin {
            server: address,
            device,
            access_token,
        } = credentials
        else {
            return Err(catalog::missing_access(PROVIDER));
        };
        let address = scoped_server(address, expected_web_root)?;
        let (user, _) = me(&address, device, access_token)?;
        Ok(Self {
            agent: agent(),
            address,
            header: auth(device, Some(access_token))?,
            user,
        })
    }

    fn request(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, ProviderError> {
        let url = at(&self.address, path);
        let mut request = self
            .agent
            .get(url.as_str())
            .header("Authorization", &self.header)
            .query("UserId", &self.user)
            .query("Fields", FIELDS);
        for (key, value) in query {
            request = request.query(*key, *value);
        }
        decode(PROVIDER, request.call())
    }
}

fn scoped_server(stored: &str, expected_web_root: Option<&Url>) -> Result<Url, ProviderError> {
    let address = server(stored)?;
    if expected_web_root.is_some_and(|root| at(&address, "web/") != *root) {
        return Err(ProviderError::new(
            ErrorKind::InvalidInput,
            "Ссылка ведёт на другой сервер Jellyfin.",
        ));
    }
    Ok(address)
}

fn linked_reference(row: &Value, id: &str) -> Option<ObjectRef> {
    let kind = match row["Type"].as_str()? {
        "MusicArtist" => ObjectKind::Artist,
        "MusicAlbum" => ObjectKind::Album,
        "Audio" => ObjectKind::Track,
        "Playlist" if matches!(row["MediaType"].as_str(), None | Some("Audio" | "Unknown")) => {
            ObjectKind::Playlist
        }
        _ => return None,
    };
    let returned_id = field(row, "Id").filter(|value| valid_item_id(value))?;
    same_item_id(&returned_id, id).then_some(ObjectRef {
        provider: PROVIDER,
        kind,
        id: returned_id,
    })
}

pub(super) fn same_item_id(left: &str, right: &str) -> bool {
    left.replace('-', "")
        .eq_ignore_ascii_case(&right.replace('-', ""))
}

fn item_type(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Artist => "MusicArtist",
        ObjectKind::Album => "MusicAlbum",
        ObjectKind::Track => "Audio",
        ObjectKind::Playlist => "Playlist",
        ObjectKind::Collection | ObjectKind::Station => "",
    }
}

fn rows(data: &Value) -> Result<&[Value], ProviderError> {
    data["Items"]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| invalid_response(PROVIDER))
}

fn from_entry(row: &Value, kind: ObjectKind) -> Option<MusicPage> {
    if row["Type"] != item_type(kind) {
        return None;
    }
    let id = field(row, "Id").filter(|id| valid_item_id(id))?;
    let reference = ObjectRef {
        provider: PROVIDER,
        kind,
        id,
    };
    let mut page = MusicPage::new(reference, field(row, "Name")?);
    page.artist_credits = playlist::artist_credits(row);
    page.artists = page
        .artist_credits
        .iter()
        .map(|credit| credit.name.clone())
        .collect();
    if page.artists.is_empty() {
        page.artists.extend(field(row, "AlbumArtist"));
    }
    page.year = row["ProductionYear"]
        .as_u64()
        .and_then(|year| year.try_into().ok());
    page.album = field(row, "Album");
    page.album_ref = playlist::album_ref(row);
    page.duration = row["RunTimeTicks"]
        .as_u64()
        .and_then(|ticks| u32::try_from(ticks / 10_000).ok());
    page.isrc = field(&row["ProviderIds"], "Isrc");
    page.barcode = field(&row["ProviderIds"], "Upc").or_else(|| field(&row["ProviderIds"], "Ean"));
    if kind == ObjectKind::Track {
        page.tracks.push(playlist::track(row, 0));
    }
    Some(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "abcdef0123456789abcdef0123456789";

    #[test]
    fn jellyfin_audio_and_album_credits_keep_only_named_server_references() {
        let lead = "11111111111111111111111111111111";
        let release = "22222222-2222-2222-2222-222222222222";
        let row = json!({
            "Type": "Audio", "Id": ID, "Name": "Recording", "Album": "Release", "AlbumId": release,
            "Artists": ["Lead", "Guest"],
            "ArtistItems": [{"Name": "Lead", "Id": lead}, {"Id": "33333333333333333333333333333333"}]
        });
        let page = from_entry(&row, ObjectKind::Track).unwrap();
        assert_eq!(page.album_ref.as_ref().unwrap().id, release);
        assert_eq!(page.artist_credits[0].reference.as_ref().unwrap().id, lead);
        assert_eq!(page.artist_credits[1].name, "Guest");
        assert!(page.artist_credits[1].reference.is_none());
        assert_eq!(page.artist_credits, page.tracks[0].artist_credits);
        assert_eq!(page.album_ref, page.tracks[0].album_ref);

        let row = json!({
            "Type": "MusicAlbum", "Id": release, "Name": "Release",
            "AlbumArtists": [{"Name": "Lead", "Id": lead}]
        });
        let page = from_entry(&row, ObjectKind::Album).unwrap();
        assert_eq!(page.artist_credits[0].reference.as_ref().unwrap().id, lead);
    }

    #[test]
    fn linked_requests_match_scheme_authority_port_and_reverse_proxy_prefix() {
        const SERVER: &str = "https://media.example:8443/jellyfin";
        let exact = Url::parse("https://media.example:8443/jellyfin/web/").unwrap();
        assert!(scoped_server(SERVER, Some(&exact)).is_ok());
        for address in [
            "http://media.example:8443/jellyfin/web/",
            "https://media.example:8444/jellyfin/web/",
            "https://media.example.invalid:8443/jellyfin/web/",
            "https://media.example:8443/web/",
            "https://media.example:8443/other/jellyfin/web/",
        ] {
            let candidate = Url::parse(address).unwrap();
            assert!(matches!(
                scoped_server(SERVER, Some(&candidate)),
                Err(ProviderError {
                    kind: ErrorKind::InvalidInput,
                    ..
                })
            ));
        }
    }

    #[test]
    fn linked_reference_type_and_identity_come_from_the_server() {
        let alternate_id = "ABCDEF01-2345-6789-ABCD-EF0123456789";
        for (label, kind) in [
            ("Audio", ObjectKind::Track),
            ("MusicAlbum", ObjectKind::Album),
            ("MusicArtist", ObjectKind::Artist),
            ("Playlist", ObjectKind::Playlist),
        ] {
            let row = json!({"Type": label, "Id": ID, "MediaType": "Audio"});
            let reference = linked_reference(&row, alternate_id).unwrap();
            assert_eq!(reference.kind, kind);
            assert_eq!(reference.id, ID);
        }
        for row in [
            json!({"Type": "Movie", "Id": ID}),
            json!({"Type": "Playlist", "MediaType": "Video", "Id": ID}),
            json!({"Type": "Audio", "Id": "11111111111111111111111111111111"}),
        ] {
            assert!(linked_reference(&row, ID).is_none());
        }
    }
}
