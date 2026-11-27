use super::*;
use crate::providers::ProviderId;

fn page(kind: ObjectKind, title: &str) -> MusicPage {
    let reference = ObjectRef {
        provider: ProviderId::Spotify,
        kind,
        id: "75UGzVK6Obcgg4TToPEJHk".into(),
    };
    let mut page = MusicPage::new(reference, title.into());
    page.artists = vec!["Исполнитель".into()];
    page
}

#[test]
fn related_artist_preview_deduplicates_legacy_cards_before_the_limit() {
    let origin =
        ObjectRef::native(ProviderId::YandexMusic, ObjectKind::Artist, "10".into()).unwrap();
    let keys = [
        "artist:10",
        "album:11",
        "artist:11",
        "artist:11",
        "artist:12",
        "artist:13",
        "artist:14",
        "artist:15",
    ];
    let cards = keys.into_iter().map(|id| HomeItem {
        id: id.into(),
        title: id.into(),
        subtitle: String::new(),
        url: None,
        artwork: None,
    });
    let shown = ArtistDetails::related_preview(&origin, cards);
    assert_eq!(
        shown
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        ["artist:11", "artist:12", "artist:13", "artist:14"],
    );
}

#[test]
fn equal_isrc_requires_compatible_recording_metadata() {
    let mut source = page(ObjectKind::Track, "Всё возможно");
    source.isrc = Some("US-ABC-24-00123".into());
    source.duration = Some(182_300);
    let mut candidate = source.clone();
    candidate.isrc = Some("USABC2400123".into());
    candidate.title = "Все возможно".into();
    candidate.duration = Some(181_800);
    assert!(same_object(&source, &candidate).is_some());
    candidate.version = Some("Live".into());
    assert!(same_object(&source, &candidate).is_none());
    candidate.version = None;
    candidate.isrc = Some("USABC2400124".into());
    assert!(same_object(&source, &candidate).is_none());
    candidate.isrc = None;
    candidate.artists.push("Другой артист".into());
    assert!(same_object(&source, &candidate).is_none());
}

#[test]
fn equal_upc_does_not_hide_a_different_tracklist() {
    let track = PlaylistTrack {
        id: "recording".into(),
        title: "Первая запись".into(),
        artist: "Исполнитель".into(),
        artists: Vec::new(),
        artist_credits: Vec::new(),
        album_ref: None,
        album: None,
        artwork: None,
        added: None,
        duration: Some(185),
        media: None,
    };
    let mut source = page(ObjectKind::Album, "Издание");
    source.barcode = Some("886443927087".into());
    source.year = Some(2024);
    source.tracks = vec![
        track.clone(),
        PlaylistTrack {
            title: "Вторая запись".into(),
            ..track
        },
    ];
    let mut candidate = source.clone();
    candidate.barcode = Some("0886443927087".into());
    assert!(same_object(&source, &candidate).is_some());
    candidate.tracks.swap(0, 1);
    assert!(same_object(&source, &candidate).is_none());
    candidate.tracks = source.tracks.clone();
    candidate.barcode = Some("0886443927001".into());
    assert!(same_object(&source, &candidate).is_none());
}
