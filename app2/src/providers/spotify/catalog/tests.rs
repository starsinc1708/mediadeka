use super::*;

#[test]
fn spotify_metadata_miss_keeps_the_known_recording_id() {
    let id = catalog_gid(&[1; 16]).unwrap();
    let row = playlist::unavailable_recording(&format!("spotify:track:{id}"));
    assert_eq!(row.id, id);
    assert!(row.media.is_none());
}

#[test]
fn spotify_track_metadata_retains_album_and_individual_artist_ids() {
    let raw_id = |n| vec![n; 16];
    let mut lead = Artist::new();
    lead.set_name("Lead & Band".into());
    lead.set_gid(raw_id(1));
    let mut guest = Artist::new();
    guest.set_name("Unlinked guest".into());
    guest.set_gid(raw_id(0));
    let mut second = Artist::new();
    second.set_name("Second guest".into());
    second.set_gid(raw_id(2));
    let mut album = Album::new();
    album.set_name("Chosen release".into());
    album.set_gid(raw_id(3));
    let mut track = Track::new();
    track.set_name("Recording".into());
    track.artist = vec![lead, guest, second];
    track.album = protobuf::MessageField::some(album);
    let id = catalog_gid(&raw_id(4)).unwrap();
    let reference = ObjectRef::native(PROVIDER, ObjectKind::Track, id).unwrap();
    let uri = SpotifyUri::from_uri(&super::uri(&reference)).unwrap();
    let page = decode_track_metadata(&reference, &uri, &track.write_to_bytes().unwrap()).unwrap();

    assert_eq!(
        page.artists,
        ["Lead & Band", "Unlinked guest", "Second guest"]
    );
    assert_eq!(
        page.artist_credits[0].reference.as_ref().unwrap().id,
        catalog_gid(&raw_id(1)).unwrap()
    );
    assert_eq!(page.artist_credits[1].reference, None);
    assert_eq!(
        page.artist_credits[2].reference.as_ref().unwrap().id,
        catalog_gid(&raw_id(2)).unwrap()
    );
    assert_eq!(
        page.album_ref.as_ref().unwrap().id,
        catalog_gid(&raw_id(3)).unwrap()
    );
    assert_eq!(page.artist_credits, page.tracks[0].artist_credits);
    assert_eq!(page.album_ref, page.tracks[0].album_ref);
}
