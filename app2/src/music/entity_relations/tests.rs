use super::*;

use crate::{
    playlist::{ArtistCredit, PlaylistTrack},
    providers::ProviderId,
};

const REGION: &str = "ZZ";
const TRACK: &str = "aabbccddaabbccddaabbccddaabbccdd";
const FIRST: &str = "11111111111111111111111111111111";
const SECOND: &str = "22222222222222222222222222222222";

fn object(kind: ObjectKind, id: &str) -> ObjectRef {
    ObjectRef::native(ProviderId::Jellyfin, kind, id.to_owned()).unwrap()
}

fn credit(name: &str, reference: Option<ObjectRef>) -> ArtistCredit {
    ArtistCredit {
        name: name.to_owned(),
        reference,
    }
}

#[test]
fn colliding_name_credits_stay_ambiguous_in_any_order() {
    let origin = object(ObjectKind::Track, TRACK);
    let artist = object(ObjectKind::Artist, FIRST);
    let other = object(ObjectKind::Artist, SECOND);
    let mut page = MusicPage::new(origin.clone(), "Запись".to_owned());
    page.artist_credits = vec![
        credit("Ёлка", Some(artist.clone())),
        credit("Елка", Some(other)),
        credit("Другой артист", Some(artist.clone())),
    ];
    assert_eq!(
        from_page(&page, &origin, ObjectKind::Artist, "елка"),
        Relation::Ambiguous
    );
    page.artist_credits.reverse();
    assert_eq!(
        from_page(&page, &origin, ObjectKind::Artist, "елка"),
        Relation::Ambiguous
    );
    page.artist_credits[1].reference = None;
    assert_eq!(
        from_page(&page, &origin, ObjectKind::Artist, "елка"),
        Relation::Ambiguous
    );
    page.artist_credits[1].reference = Some(artist.clone());
    assert_eq!(
        from_page(&page, &origin, ObjectKind::Artist, "елка"),
        Relation::Unique(artist)
    );
}

#[test]
fn only_the_origin_track_row_can_supply_an_album_id() {
    let origin = object(ObjectKind::Track, TRACK);
    let album = object(ObjectKind::Album, FIRST);
    let other = object(ObjectKind::Album, SECOND);
    let row = |id: &str, reference: &ObjectRef| PlaylistTrack {
        id: id.to_owned(),
        title: "Запись".to_owned(),
        artist: String::new(),
        artists: Vec::new(),
        artist_credits: Vec::new(),
        album: Some("Издание".to_owned()),
        album_ref: Some(reference.clone()),
        artwork: None,
        added: None,
        duration: None,
        media: None,
    };
    let mut page = MusicPage::new(origin.clone(), "Запись".to_owned());
    page.album = Some("Издание".to_owned());
    page.tracks = vec![row(SECOND, &other), row(TRACK, &album)];
    assert_eq!(
        from_page(&page, &origin, ObjectKind::Album, "издание"),
        Relation::Unique(album)
    );
    page.album_ref = Some(other);
    assert_eq!(
        from_page(&page, &origin, ObjectKind::Album, "издание"),
        Relation::Ambiguous
    );
}

#[test]
fn stale_catalog_credit_is_available_without_connection_and_cancellation_is_respected() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let origin = object(ObjectKind::Track, TRACK);
    let artist = object(ObjectKind::Artist, FIRST);
    let mut page = MusicPage::new(origin.clone(), "Запись".to_owned());
    page.artist_credits
        .push(credit("Ёлка", Some(artist.clone())));
    assert!(
        store
            .save_music_snapshot(
                origin.provider,
                None,
                REGION,
                &format!("track:{TRACK}"),
                &page
            )
            .unwrap()
    );
    rusqlite::Connection::open(directory.path().join("mediadeka.sqlite3"))
        .unwrap()
        .execute("UPDATE music_snapshot SET saved_at_ms = 0", [])
        .unwrap();

    let query = PageQuery::resolve(&store, &origin, None, REGION).unwrap();
    assert!(!query.cached().unwrap().unwrap().fresh(ObjectKind::Track));
    let ongoing = AtomicBool::new(false);
    assert_eq!(
        resolve(&query, &origin, ObjectKind::Artist, "елка", &ongoing),
        Some(artist)
    );
    // Jellyfin is unavailable without credentials. A failed refresh cannot attach another artist.
    assert_eq!(
        resolve(&query, &origin, ObjectKind::Artist, "другой", &ongoing),
        None
    );
    assert_eq!(query.cached().unwrap().unwrap().data.as_ref(), &page);
    assert_eq!(
        resolve(
            &query,
            &origin,
            ObjectKind::Artist,
            "елка",
            &AtomicBool::new(true)
        ),
        None
    );
    assert!(PageQuery::resolve(&store, &origin, Some("other-profile"), REGION).is_err());
}
