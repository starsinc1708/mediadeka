use super::{
    Preferences, Service, Store,
    scrobble::{LastFmAccount, ListenBrainzAccount},
    submission::Account,
};

mod windows;

#[test]
fn listening_preferences_and_accounts_survive_profile_reopen() {
    let profile = tempfile::tempdir().unwrap();
    let store = Store::versioned_fixture(profile.path(), 23).unwrap();
    drop(store);
    let store = Store::open(profile.path()).unwrap();
    assert_eq!(
        store.listening_preferences().unwrap(),
        Preferences::default()
    );

    let preferences = Preferences {
        discord_enabled: true,
        discord_paused: true,
        discord_private: true,
        discord_application_id: "42".into(),
        ..Default::default()
    };
    store.set_listening_preferences(&preferences).unwrap();
    let lastfm = Account::LastFm(LastFmAccount::test_at("http://127.0.0.1:1".into()));
    let listenbrainz =
        Account::ListenBrainz(ListenBrainzAccount::test_at("http://127.0.0.1:1".into()));
    Account::save(&store, Service::LastFm, Some(&lastfm)).unwrap();
    Account::save(&store, Service::ListenBrainz, Some(&listenbrainz)).unwrap();
    drop(store);

    let store = Store::open(profile.path()).unwrap();
    let restored_preferences = store.listening_preferences();
    let restored_lastfm = Account::load(&store, Service::LastFm);
    let restored_listenbrainz = Account::load(&store, Service::ListenBrainz);
    Account::save(&store, Service::LastFm, None).unwrap();
    let unaffected = Account::load(&store, Service::ListenBrainz);
    Account::save(&store, Service::ListenBrainz, None).unwrap();

    assert_eq!(restored_preferences.unwrap(), preferences);
    assert!(matches!(restored_lastfm.unwrap(), Some(Account::LastFm(_))));
    assert!(matches!(
        restored_listenbrainz.unwrap(),
        Some(Account::ListenBrainz(_))
    ));
    assert!(matches!(
        unaffected.unwrap(),
        Some(Account::ListenBrainz(_))
    ));
    assert!(Account::load(&store, Service::LastFm).unwrap().is_none());
    assert!(
        Account::load(&store, Service::ListenBrainz)
            .unwrap()
            .is_none()
    );
}
