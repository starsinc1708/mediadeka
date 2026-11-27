use std::sync::atomic::AtomicBool;
use std::{env, fs, process::Command};

use gpui::TestAppContext;
use rusqlite::Connection;

use super::*;
use crate::{
    appearance::Appearance,
    music::likes::Subject,
    playlist::{Media, PlaylistTrack},
    ui::likes::LikeAction,
};

fn rerun_in_isolated_profile(test: &str) -> bool {
    const FIXTURE_PROCESS: &str = "MEDIADEKA_TEST_FAVORITES_FIXTURE";
    if env::var_os(FIXTURE_PROCESS).is_some() {
        return false;
    }
    let profile = tempfile::tempdir().unwrap();
    let output = Command::new(env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env(FIXTURE_PROCESS, "1")
        .env("MEDIADEKA_PROFILE_DIR", profile.path())
        .output()
        .unwrap();
    assert!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    true
}

#[test]
fn retry_restores_all_saved_favorites_before_enabling_changes() {
    if rerun_in_isolated_profile(concat!(
        "ui::playlist::favorites::tests::",
        "retry_restores_all_saved_favorites_before_enabling_changes"
    )) {
        return;
    }

    let profile = Store::profile_directory().unwrap();
    let store = Store::open(&profile).unwrap();
    let source = profile.join("favorites.m3u");
    fs::write(
        &source,
        "#EXTM3U\n#EXTINF:1,First\nfirst.mp3\n#EXTINF:1,Second\nsecond.mp3\n#EXTINF:1,Third\nthird.mp3\n",
    )
    .unwrap();
    let id = store.create_local_playlist("Favorites", &[source]).unwrap();
    let key = PlaylistKey::Local(id);
    let playlist = store.local_playlist(id).unwrap().unwrap();
    let first = &playlist.tracks[0].id;
    let second = &playlist.tracks[1].id;
    let third = &playlist.tracks[2].id;
    let subject = Subject::new(&key, &playlist.tracks[2]).unwrap();
    for track in [first, second] {
        assert!(
            store
                .set_track_favorite(FavoriteScope::Local, track, true, &AtomicBool::new(false))
                .unwrap()
        );
    }
    let saved = HashSet::from([first.clone(), second.clone()]);

    let connection = Connection::open(profile.join("mediadeka.sqlite3")).unwrap();
    connection
        .execute_batch("ALTER TABLE favorite_track RENAME TO saved_favorite_track")
        .unwrap();
    let mut context = TestAppContext::single();
    let window = context.add_window(|window, cx| {
        crate::ui::theme::install(cx, false);
        cx.bind_keys([
            gpui::KeyBinding::new("escape", crate::ui::Escape, None),
            gpui::KeyBinding::new("tab", crate::ui::Tab, None),
            gpui::KeyBinding::new("shift-tab", crate::ui::ShiftTab, None),
        ]);
        MainView::new(window, cx, false, Some(true), false, Appearance::classic())
    });
    window
        .update(&mut context, |view, window, cx| {
            view.open_local_playlist_for_capture(id, window, cx);
        })
        .unwrap();
    context.run_until_parked();
    window
        .update(&mut context, |view, _, _| {
            assert_eq!(view.playlist_status_for_capture(), Some(Ok(3)));
            assert!(view.playlist.favorites.read_failed(&key));
            assert_eq!(view.playlist.favorites.mark(&key, first), None);
        })
        .unwrap();

    window
        .update(&mut context, |view, window, cx| {
            view.open_like_destinations(subject.clone(), true, window, cx);
        })
        .unwrap();
    context.run_until_parked();

    connection
        .execute_batch("ALTER TABLE saved_favorite_track RENAME TO favorite_track")
        .unwrap();
    assert_eq!(store.favorite_tracks(FavoriteScope::Local).unwrap(), saved);
    window
        .update(&mut context, |view, window, cx| {
            view.close_like_menu(window, cx);
        })
        .unwrap();

    #[cfg(debug_assertions)]
    {
        let mut visual = gpui::VisualTestContext::from_window(window.into(), &context);
        let retry = visual
            .debug_bounds("playlist-favorites-retry")
            .expect("failed reads offer a retry in the rendered playlist");
        visual.simulate_click(retry.center(), gpui::Modifiers::default());
    }
    #[cfg(not(debug_assertions))]
    window
        .update(&mut context, |view, _, cx| {
            view.retry_playlist_favorites(&key, cx)
        })
        .unwrap();
    context.run_until_parked();
    window
        .update(&mut context, |view, window, cx| {
            let marks = &view.playlist.favorites;
            assert!(!marks.read_failed(&key));
            assert_eq!(marks.mark(&key, first), Some(true));
            assert_eq!(marks.mark(&key, second), Some(true));
            assert_eq!(marks.mark(&key, third), Some(false));
            view.open_like_destinations(subject, true, window, cx);
        })
        .unwrap();
    context.run_until_parked();
    assert_eq!(
        store.favorite_tracks(FavoriteScope::Local).unwrap(),
        HashSet::from([first.clone(), second.clone(), third.clone()])
    );

    #[cfg(debug_assertions)]
    {
        let mut visual = gpui::VisualTestContext::from_window(window.into(), &context);
        let heart = visual.debug_bounds("playlist-like-2").unwrap().center();
        let modifiers = gpui::Modifiers::default();
        visual.simulate_mouse_down(heart, gpui::MouseButton::Right, modifiers);
        visual.simulate_mouse_up(heart, gpui::MouseButton::Right, modifiers);
        context.run_until_parked();
        assert!(visual.debug_bounds("like-destinations-menu").is_some());
        assert!(visual.debug_bounds("like-destination-mediadeka").is_some());
        for keys in ["tab", "tab", "shift-tab"] {
            visual.simulate_keystrokes(keys);
            window
                .update(&mut context, |view, window, cx| {
                    assert!(view.like_menu_focus().unwrap().contains_focused(window, cx));
                })
                .unwrap();
        }
        visual.simulate_keystrokes("escape");
        context.run_until_parked();
        assert!(visual.debug_bounds("like-destinations-menu").is_none());
        visual.simulate_mouse_down(heart, gpui::MouseButton::Left, modifiers);
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(500));
        context.run_until_parked();
        assert!(visual.debug_bounds("like-destinations-menu").is_some());
        visual.simulate_mouse_up(heart, gpui::MouseButton::Left, modifiers);
        assert!(
            store
                .favorite_tracks(FavoriteScope::Local)
                .unwrap()
                .contains(third)
        );
    }
}

#[test]
fn original_online_track_uses_saved_mediadeka_mark_after_reopen() {
    if rerun_in_isolated_profile(concat!(
        "ui::playlist::favorites::tests::",
        "original_online_track_uses_saved_mediadeka_mark_after_reopen"
    )) {
        return;
    }

    let profile = Store::profile_directory().unwrap();
    let provider = ProviderId::YouTubeMusic;
    let key = PlaylistKey::Source {
        provider,
        profile: None,
        id: "fixture-playlist".into(),
    };
    let track = PlaylistTrack {
        id: "a1b2c3D4e5F".into(),
        title: "Previously saved track".into(),
        artist: "Fixture artist".into(),
        artists: Vec::new(),
        artist_credits: Vec::new(),
        album_ref: None,
        album: None,
        artwork: None,
        added: None,
        duration: Some(60),
        media: Some(Media::Provider {
            provider,
            id: "a1b2c3D4e5F".into(),
        }),
    };
    let subject = Subject::new(&key, &track).unwrap();
    let store = Store::open(&profile).unwrap();
    store
        .set_local_like(&subject.identity, &track, true, &AtomicBool::new(false))
        .unwrap();
    drop(store);
    let reopened = Store::open(&profile).unwrap();
    assert!(reopened.local_like(&subject.identity).unwrap());

    let mut context = TestAppContext::single();
    let window = context.add_window(|window, cx| {
        crate::ui::theme::install(cx, false);
        MainView::new(window, cx, false, Some(true), false, Appearance::classic())
    });
    window
        .update(&mut context, |view, _, cx| {
            view.ensure_playlist_favorites(&key, cx);
        })
        .unwrap();
    context.run_until_parked();
    window
        .update(&mut context, |view, window, cx| {
            assert_eq!(
                view.track_like_action(&key, &track, &[]),
                Some(LikeAction::Remove)
            );
            view.open_like_destinations(subject.clone(), true, window, cx);
        })
        .unwrap();
    context.run_until_parked();
    assert!(!reopened.local_like(&subject.identity).unwrap());
    window
        .update(&mut context, |view, window, cx| {
            // A native mark outside the defaults must still offer saving to Mediadeka.
            assert_eq!(
                view.track_like_action(&key, &track, &[Some(provider)]),
                Some(LikeAction::Save)
            );
            view.open_like_destinations(subject.clone(), true, window, cx);
        })
        .unwrap();
    context.run_until_parked();
    assert!(reopened.local_like(&subject.identity).unwrap());
}
