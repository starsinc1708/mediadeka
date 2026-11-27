use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

#[cfg(windows)]
pub(crate) mod audio;
#[cfg(windows)]
mod catalog;
mod collections;
mod playlists;
#[cfg(windows)]
mod recovery;
#[cfg(windows)]
pub(super) use playlists::profile_snapshot;

#[test]
#[ignore = "calls the real public YouTube Music service"]
fn live_guest_home() {
    let home =
        guest_home(&std::sync::atomic::AtomicBool::new(false)).expect("YouTube Music public home");
    assert!(
        home.sections.len() > 1,
        "YouTube Music returned a single shelf"
    );
    assert!(!home.items.is_empty());
    assert!(home.items.iter().all(|item| item.url.is_some()));
    println!(
        "YouTube Music public home: {} musical entries",
        home.items.len()
    );
}

#[test]
#[ignore = "requires an explicitly supplied signed-in YouTube Music Cookie header"]
fn live_authenticated_youtube_profile() {
    let cookies =
        std::env::var("MEDIADEKA_TEST_YOUTUBE_COOKIE").expect("test YouTube Music cookie");
    let cancelled = AtomicBool::new(false);
    let choices = youtube_profiles(cookies, &cancelled).expect("list YouTube Music profiles");
    let id = choices
        .profiles()
        .first()
        .expect("signed-in profile")
        .id
        .clone();
    let session = sign_in(
        ProviderId::YouTubeMusic,
        LoginInput::ChooseYouTube { choices, id },
        &cancelled,
        |_| {},
    )
    .unwrap_or_else(|error| panic!("YouTube Music profile check: {error}"));
    let verified = restore(
        session.provider,
        &session.profile,
        session.credentials.clone(),
        &cancelled,
    )
    .expect("restore YouTube Music session");
    assert_eq!(session.profile, verified.profile);
    println!(
        "YouTube Music account choices and restoration verified; {} musical entries",
        home(&verified, &cancelled)
            .expect("YouTube Music account feed")
            .items
            .len()
    );
}

#[test]
#[ignore = "requests a real Yandex device code but does not authenticate"]
fn live_yandex_device_start_and_cancel() {
    let cancelled = AtomicBool::new(false);
    let result = sign_in(
        ProviderId::YandexMusic,
        LoginInput::YandexDevice,
        &cancelled,
        |prompt| {
            let LoginPrompt::Code { code, url } = prompt else {
                panic!("expected Yandex device prompt")
            };
            assert!(!code.is_empty());
            assert!(url.starts_with("https://"));
            cancelled.store(true, Ordering::Relaxed);
        },
    );
    expect_cancelled(result);
    println!("Yandex Music returned a device challenge; application cancelled it");
}

#[test]
#[ignore = "loads the real Spotify authorization page; no browser session changes"]
fn live_spotify_browser_start_and_cancel() {
    let cancelled = AtomicBool::new(false);
    let result = sign_in(
        ProviderId::Spotify,
        LoginInput::Spotify,
        &cancelled,
        |prompt| {
            let LoginPrompt::Browser { url } = prompt else {
                panic!("expected Spotify browser prompt")
            };
            let response = transport::agent()
                .get(&url)
                .call()
                .expect("Spotify authorization endpoint");
            println!(
                "Spotify authorization start: HTTP {}",
                response.status().as_u16()
            );
            assert!(response.status().is_success() || response.status().is_redirection());
            cancelled.store(true, Ordering::Relaxed);
        },
    );
    expect_cancelled(result);
    let restarted = sign_in(
        ProviderId::Spotify,
        LoginInput::Spotify,
        &cancelled,
        |_| unreachable!(),
    );
    expect_cancelled(restarted);
}

#[test]
#[ignore = "uses a dedicated Jellyfin server with a test music library"]
fn live_jellyfin_login_and_restore() {
    let server = std::env::var("MEDIADEKA_TEST_JELLYFIN_URL").expect("fixture URL");
    let username = std::env::var("MEDIADEKA_TEST_JELLYFIN_USER").expect("fixture username");
    let password = std::env::var("MEDIADEKA_TEST_JELLYFIN_PASSWORD").expect("fixture password");
    let cancelled = AtomicBool::new(false);
    let provider = ProviderId::Jellyfin;
    let session = sign_in(
        provider,
        LoginInput::JellyfinPassword {
            server: server.clone(),
            username,
            password,
        },
        &cancelled,
        |_| {},
    )
    .unwrap_or_else(|error| panic!("Jellyfin password sign-in: {error}"));
    let restored = restore(
        provider,
        &session.profile,
        session.credentials.clone(),
        &cancelled,
    )
    .unwrap_or_else(|error| panic!("Jellyfin password restore: {error}"));
    assert_eq!(session.profile, restored.profile);
    let home = home(&restored, &cancelled).expect("load Jellyfin music");
    assert!(!home.items.is_empty());
    let quick = || LoginInput::JellyfinQuickConnect {
        server: server.clone(),
    };
    let interrupted = sign_in(provider, quick(), &cancelled, |prompt| {
        assert!(matches!(prompt, LoginPrompt::Code { .. }));
        cancelled.store(true, Ordering::Relaxed);
    });
    expect_cancelled(interrupted);
    cancelled.store(false, Ordering::Relaxed);
    let remote_session = sign_in(provider, quick(), &cancelled, |prompt| {
        let LoginPrompt::Code { code, .. } = prompt else {
            panic!("expected Jellyfin Quick Connect challenge")
        };
        let Credentials::Jellyfin { access_token, device, .. } = &session.credentials else {
            unreachable!()
        };
        let authorization = format!("MediaBrowser Client=\"Mediadeka\", Device=\"Windows\", DeviceId=\"{device}\", Version=\"{}\", Token=\"{access_token}\"", env!("CARGO_PKG_VERSION"));
        let response = transport::agent()
            .post(&format!("{server}/QuickConnect/Authorize"))
            .query("code", &code)
            .header("Authorization", authorization)
            .send_empty()
            .expect("approve fixture challenge");
        assert!(
            response.status().is_success(),
            "approve challenge HTTP {}",
            response.status().as_u16()
        );
    })
    .unwrap_or_else(|error| panic!("Jellyfin Quick Connect sign-in: {error}"));
    let restored = restore(
        provider,
        &remote_session.profile,
        remote_session.credentials,
        &cancelled,
    )
    .expect("restore Quick Connect account");
    assert_eq!(remote_session.profile, restored.profile);
    assert_eq!(session.profile, restored.profile);
    println!(
        "Jellyfin password and Quick Connect verified; same account restored; {} musical entries",
        home.items.len()
    );
}

#[cfg(windows)]
#[test]
#[ignore = "reads the existing v1 SoundCloud connection via Credential Manager and verifies it remotely"]
fn live_existing_soundcloud_sign_in_and_restore() {
    use crate::storage::{Startup, Store};

    let Some(data) = prior_connection("My Media/SoundCloud") else {
        panic!("no existing SoundCloud connection");
    };
    let token = data["token"]
        .as_str()
        .expect("v1 SoundCloud session format");
    let cancelled = AtomicBool::new(false);
    let provider = ProviderId::SoundCloud;
    let client_id = data["client_id"]
        .as_str()
        .expect("v1 SoundCloud client format");
    let session = soundcloud::sign_in(token.into(), &cancelled)
        .unwrap_or_else(|error| panic!("SoundCloud profile check: {error}"));
    let prior = soundcloud::restore(
        Credentials::SoundCloud {
            token: token.into(),
            client_id: client_id.into(),
        },
        &cancelled,
    )
    .unwrap_or_else(|error| panic!("original SoundCloud session check: {error}"));
    assert_eq!(session.profile, prior.profile);

    let directory = tempfile::tempdir().expect("isolated Mediadeka profile");
    let store = Store::open(directory.path()).unwrap();
    assert!(store.use_account(&session, || Some(()), || {}).unwrap());
    drop(store);
    let reopened = Store::open(directory.path()).unwrap();
    let Startup::Account(account) = reopened.startup().unwrap() else {
        panic!("SoundCloud account was not restored from the isolated profile");
    };
    assert_eq!(account.provider, provider);
    let credentials = reopened.credentials(&account);
    // Remove the isolated Credential Manager entry even if live revalidation fails.
    reopened
        .disconnect_account(provider, &account.profile.id, || Some(()))
        .unwrap();
    let restored = restore(
        provider,
        &account.profile,
        credentials.unwrap().unwrap(),
        &cancelled,
    )
    .unwrap_or_else(|error| panic!("restore saved SoundCloud session: {error}"));
    assert_eq!(restored.profile, session.profile);
    println!("SoundCloud profile was confirmed and restored from an isolated profile");
}

#[cfg(windows)]
#[test]
#[ignore = "reads the existing v1 Yandex Music connection via Credential Manager and verifies it remotely"]
fn live_existing_yandex_restore() {
    let session = existing_yandex_session();
    let home = home(&session, &AtomicBool::new(false)).expect("Yandex Music feed");
    assert!(
        !home.items.is_empty(),
        "expected music in the existing test profile"
    );
    for (title, minimum_tabs) in [
        ("В стиле", 2),
        ("Волна открытий", 6),
        ("Новые релизы", 2),
        ("Подборки музыки", 0),
        ("Вы могли пропустить", 0),
        ("Больше новой музыки", 0),
    ] {
        let section = home
            .sections
            .iter()
            .find(|section| section.title == title)
            .unwrap_or_else(|| panic!("missing Yandex Music shelf: {title}"));
        assert!(
            section.len > 0 && section.tabs.len() >= minimum_tabs,
            "{title}"
        );
        let items = &home.items[section.start..section.start + section.len];
        assert!(items.iter().any(|item| item.artwork.is_some()), "{title}");
        if title != "Волна открытий" {
            assert!(items.iter().any(|item| item.url.is_some()), "{title}");
        }
    }
    println!(
        "Yandex Music profile restored; {} shelves, {} entries, {} artwork links",
        home.sections.len(),
        home.items.len(),
        home.items
            .iter()
            .filter(|item| item.artwork.is_some())
            .count()
    );
    for section in &home.sections {
        println!(
            "{}: {}; filters: {}",
            section.title,
            section.len,
            section.tabs.len()
        );
    }
}

#[cfg(windows)]
pub(crate) fn existing_yandex_session() -> Session {
    use std::path::PathBuf;
    let profile =
        PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA")).join("My Media");
    let Some(data) = prior_connection(&format!("My Media/Yandex Music/{}", profile.display()))
    else {
        panic!("no existing Yandex Music connection");
    };
    let access_token = data["token"]
        .as_str()
        .expect("v1 Yandex Music session format")
        .into();
    yandex::restore(
        Credentials::YandexMusic {
            access_token,
            refresh_token: None,
            access_expires_at: None,
        },
        &AtomicBool::new(false),
    )
    .unwrap_or_else(|error| panic!("Yandex Music profile check: {error}"))
}

#[cfg(windows)]
fn prior_connection(target: &str) -> Option<serde_json::Value> {
    use std::{ptr, slice};
    use windows_sys::Win32::Security::Credentials::{
        CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW,
    };

    let target: Vec<u16> = target.encode_utf16().chain([0]).collect();
    let mut raw: *mut CREDENTIALW = ptr::null_mut();
    // SAFETY: Windows owns the returned buffer until CredFree; the target is terminated.
    if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut raw) } == 0 {
        return None;
    }
    // SAFETY: A successful CredReadW returned a live record and owns its blob, which is parsed here then freed.
    unsafe {
        let credential = &*raw;
        let result = if credential.CredentialBlobSize == 0 || credential.CredentialBlob.is_null() {
            None
        } else {
            serde_json::from_slice(slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            ))
            .ok()
        };
        CredFree(raw.cast());
        result
    }
}

fn expect_cancelled(result: Result<Session, ProviderError>) {
    match result {
        Err(error) => assert_eq!(error.kind, ErrorKind::Cancelled, "{error}"),
        Ok(_) => panic!("cancelled sign-in returned a session"),
    }
}
