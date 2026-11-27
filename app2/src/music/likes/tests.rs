use super::*;

fn track(id: &str, media: Option<Media>) -> PlaylistTrack {
    PlaylistTrack {
        id: id.into(),
        title: "Mediadeka fixture recording".into(),
        artist: "Mediadeka".into(),
        artists: Vec::new(),
        artist_credits: Vec::new(),
        album_ref: None,
        album: None,
        artwork: None,
        added: None,
        duration: Some(4),
        media,
    }
}

#[test]
fn a_saved_online_copy_preserves_its_original_identity_and_preferences() {
    let profile = tempfile::tempdir().unwrap();
    let store = Store::open(profile.path()).unwrap();
    let provider = ProviderId::Jellyfin;
    let key = PlaylistKey::Source {
        provider,
        profile: Some("jellyfin-fixture-account".into()),
        id: "test".into(),
    };
    let id = "29ef048cedd44ff8b81c413058f60d00";
    let track = track(
        id,
        Some(Media::Provider {
            provider,
            id: id.into(),
        }),
    );
    let subject = Subject::new(&key, &track).unwrap();
    let cancelled = AtomicBool::new(false);
    let defaults = [Some(provider), None];
    store.set_like_defaults(&defaults).unwrap();
    set_local(&store, &subject, true, &cancelled).unwrap();
    drop(store);

    let store = Store::open(profile.path()).unwrap();
    assert_eq!(store.like_defaults().unwrap(), defaults);
    let saved = store.local_liked_tracks().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].title, track.title);
    assert_eq!(saved[0].media, track.media);
    assert!(saved[0].added.is_some());
    assert_eq!(
        LikeIdentity::from_track(&PlaylistKey::Local(0), &saved[0]),
        Some(subject.identity.clone())
    );
    assert!(store.local_like(&subject.identity).unwrap());
    set_local(&store, &subject, false, &cancelled).unwrap();
    assert!(store.local_liked_tracks().unwrap().is_empty());
}

#[cfg(windows)]
#[test]
#[ignore = "mutates a new track in an isolated Jellyfin fixture and removes the mark"]
fn live_jellyfin_and_mediadeka_favorites() {
    use crate::providers::{
        LoginInput,
        collections::{self, CollectionKind},
        read_playlist, sign_in,
    };

    const REOPEN_PROFILE: &str = "MEDIADEKA_TEST_JELLYFIN_LIKE_REOPEN_PROFILE";
    const REOPEN_TRACK: &str = "MEDIADEKA_TEST_JELLYFIN_LIKE_REOPEN_TRACK";
    if let Some(profile) = std::env::var_os(REOPEN_PROFILE) {
        let store = Store::open(profile).unwrap();
        let account = store
            .preferred_account(ProviderId::Jellyfin)
            .unwrap()
            .unwrap();
        let credentials = store.credentials(&account).unwrap().unwrap();
        let id = std::env::var(REOPEN_TRACK).unwrap();
        let Collection::Liked(tracks) = collections::read(
            &credentials,
            &account.profile,
            CollectionKind::Liked,
            &AtomicBool::new(false),
        )
        .unwrap() else {
            panic!("Jellyfin returned a different collection");
        };
        assert!(tracks.iter().any(|track| track.id == id));
        assert!(
            store
                .local_liked_tracks()
                .unwrap()
                .iter()
                .any(|track| track.id == "local-copy")
        );
        assert_eq!(
            store.remote_like_override(&account, &id).unwrap(),
            Some(true)
        );
        return;
    }

    let cancelled = AtomicBool::new(false);
    let required = |name| std::env::var(name).expect("isolated Jellyfin fixture variable");
    let url = required("MEDIADEKA_TEST_JELLYFIN_URL");
    assert!(
        url::Url::parse(&url)
            .unwrap()
            .host_str()
            .is_some_and(|host| host == "127.0.0.1")
    );
    let provider = ProviderId::Jellyfin;
    let session = sign_in(
        provider,
        LoginInput::JellyfinPassword {
            server: url,
            username: required("MEDIADEKA_TEST_JELLYFIN_USER"),
            password: required("MEDIADEKA_TEST_JELLYFIN_PASSWORD"),
        },
        &cancelled,
        |_| {},
    )
    .unwrap();
    let read_liked = || {
        let Collection::Liked(tracks) = collections::read(
            &session.credentials,
            &session.profile,
            CollectionKind::Liked,
            &cancelled,
        )
        .unwrap() else {
            panic!("Jellyfin expected a track collection");
        };
        tracks
    };
    let playlist = required("MEDIADEKA_TEST_JELLYFIN_PLAYLIST");
    let before = read_liked();
    let initial =
        read_playlist(provider, Some(&session.credentials), &playlist, &cancelled).unwrap();
    let track = initial
        .tracks
        .into_iter()
        .find(|track| track.media.is_some() && before.iter().all(|liked| liked.id != track.id))
        .expect("an initially unliked test recording");
    let profile = tempfile::tempdir().unwrap();
    let store = Store::open(profile.path()).unwrap();
    store.use_account(&session, || Some(()), || {}).unwrap();
    let id = track.id.clone();
    // Use a local copy to exercise catalog search and explicit confirmation before writing.
    let mut local = track.clone();
    local.id = "local-copy".into();
    local.media = Some(Media::File(profile.path().join("fixture.wav")));
    let subject = Subject::new(&PlaylistKey::Local(0), &local).unwrap();
    let remote = || {
        destinations(&store, &subject)
            .unwrap()
            .into_iter()
            .find(|row| row.provider == Some(provider))
            .unwrap()
    };
    let destination = remote();
    assert!(matches!(destination.state, State::Unmatched));
    let candidates =
        candidates(&store, &subject, provider, &session.profile.id, &cancelled).unwrap();
    let recording = candidates
        .into_iter()
        .find(|recording| recording.reference.id == id)
        .unwrap();
    set_local(&store, &subject, true, &cancelled).unwrap();
    set_remote(
        &store,
        &subject,
        &destination,
        &recording,
        true,
        true,
        &cancelled,
    )
    .unwrap();

    let reopened = Store::open(profile.path()).unwrap();
    let rows = destinations(&reopened, &subject).unwrap();
    let confirmed = reopened.confirmed_track_pairs().unwrap();
    let server_saved = read_liked().into_iter().any(|track| track.id == id);
    let membership = check_remote(&reopened, &destination, &recording, &cancelled);
    let new_process = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "music::likes::tests::live_jellyfin_and_mediadeka_favorites",
            "--ignored",
        ])
        .env(REOPEN_PROFILE, profile.path())
        .env(REOPEN_TRACK, &id)
        .output();
    let mut rejected_track = recording.clone();
    rejected_track.reference.id = "00000000000000000000000000000001".into();
    let rejected = set_remote(
        &reopened,
        &subject,
        &destination,
        &rejected_track,
        true,
        false,
        &cancelled,
    );
    let mapping_retained = reopened.confirmed_track_pairs().unwrap() == confirmed;
    // Always remove the fixture's remote mark before validating recorded evidence.
    set_remote(
        &reopened,
        &subject,
        &destination,
        &recording,
        false,
        true,
        &cancelled,
    )
    .unwrap();
    let server_removed = read_liked().into_iter().all(|track| track.id != id);
    let local_retained = reopened.local_like(&subject.identity).unwrap();
    let _ = reopened.disconnect_account(provider, &session.profile.id, || Some(()));

    assert!(server_saved && server_removed);
    assert!(membership.unwrap());
    let new_process = new_process.unwrap();
    assert!(
        new_process.status.success(),
        "{}{}",
        String::from_utf8_lossy(&new_process.stdout),
        String::from_utf8_lossy(&new_process.stderr)
    );
    assert!(mapping_retained);
    assert_eq!(confirmed.len(), 1);
    assert!(matches!(rows[0].state, State::Local { liked: true }));
    assert!(rows.iter().any(|row| matches!(
        row.state,
        State::Ready {
            liked: Some(true),
            ..
        }
    )));
    assert!(rejected.is_err());
    assert!(local_retained);
    println!(
        "Jellyfin и Mediadeka: поиск, выбор совпадения, две отметки, чтение SQLite и сервера в новом процессе, отдельный отказ и снятие тестового лайка."
    );
}
