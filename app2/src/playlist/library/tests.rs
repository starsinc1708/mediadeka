use super::*;

#[test]
fn local_library_reports_missing_playlists_and_persists_changes() {
    let profile = tempfile::tempdir().unwrap();
    let store = Store::open(profile.path()).unwrap();
    let LocalChanged::Created(id, _) = (LocalChange::Create {
        name: "Прежнее имя".into(),
        files: Vec::new(),
    })
    .apply(&store)
    .unwrap() else {
        panic!("Expected a created playlist");
    };
    assert_eq!(local_playlist(&store, id).unwrap().name, "Прежнее имя");

    LocalChange::Rename {
        id,
        name: "Новое имя".into(),
    }
    .apply(&store)
    .unwrap();
    let reopened = Store::open(profile.path()).unwrap();
    assert_eq!(local_playlist(&reopened, id).unwrap().name, "Новое имя");

    LocalChange::Delete(id).apply(&store).unwrap();
    assert!(matches!(
        local_playlist(&reopened, id),
        Err(ReadError::Removed)
    ));
    assert!(matches!(
        LocalChange::Delete(id).apply(&store),
        Err(ChangeError::Removed)
    ));
}

#[test]
fn source_reads_keep_provider_error_kinds() {
    assert!(
        source_credentials(ProviderId::YouTubeMusic, None)
            .unwrap()
            .is_none()
    );
    let invalid = PlaylistKey::Source {
        provider: ProviderId::YouTubeMusic,
        profile: None,
        id: "?".into(),
    };
    for (cancelled, expected) in [
        (false, ErrorKind::InvalidInput),
        (true, ErrorKind::Cancelled),
    ] {
        let error = read_playlist(&invalid, &AtomicBool::new(cancelled)).unwrap_err();
        assert!(matches!(error, ReadError::Provider(error) if error.kind == expected));
    }
}

#[cfg(windows)]
#[test]
fn credentials_require_the_same_provider_and_profile() {
    use crate::providers::{Capabilities, Profile, Session};

    let profile = tempfile::tempdir().unwrap();
    let store = Store::open(profile.path()).unwrap();
    let provider = ProviderId::Jellyfin;
    let id = "playlist-test-profile";
    let credentials = Credentials::Jellyfin {
        server: "http://127.0.0.1".into(),
        device: "playlist-test-device".into(),
        access_token: "playlist-test-token".into(),
    };
    let session = Session {
        provider,
        profile: Profile {
            id: id.into(),
            name: "Playlist test".into(),
        },
        credentials: credentials.clone(),
        capabilities: Capabilities {
            home: true,
            profile_selection: false,
            device_code: false,
        },
    };
    assert!(store.use_account(&session, || Some(()), || {}).unwrap());
    let exact = connected_credentials(&store, provider, id);
    let wrong_profile = connected_credentials(&store, provider, "another-profile");
    let wrong_provider = connected_credentials(&store, ProviderId::SoundCloud, id);
    store.disconnect_account(provider, id, || Some(())).unwrap();

    assert!(exact.is_ok_and(|saved| saved == credentials));
    for error in [
        wrong_profile,
        wrong_provider,
        connected_credentials(&store, provider, id),
    ] {
        assert!(matches!(error, Err(AccessError::Disconnected)));
    }
}
