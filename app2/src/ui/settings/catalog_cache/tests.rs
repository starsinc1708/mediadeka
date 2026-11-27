use std::{env, process::Command};

use gpui::TestAppContext;

use crate::{appearance::Appearance, providers::ProviderId, storage::Store};

use super::*;

#[cfg(windows)]
mod native;

#[test]
fn catalog_clear_command_clears_only_rebuildable_snapshots() {
    const CHILD: &str = "MEDIADEKA_CATALOG_CLEAR_SETTINGS_CHILD";
    if env::var_os(CHILD).is_none() {
        let directory = tempfile::tempdir().unwrap();
        let test = concat!(
            module_path!(),
            "::catalog_clear_command_clears_only_rebuildable_snapshots"
        );
        let output = Command::new(env::current_exe().unwrap())
            .args(["--exact", test.split_once("::").unwrap().1, "--nocapture"])
            .env(CHILD, "1")
            .env("MEDIADEKA_PROFILE_DIR", directory.path())
            .output()
            .unwrap();
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let store = Store::open_default().unwrap();
    let provider = ProviderId::YandexMusic;
    let region = "ZZ";
    let playlist = store
        .create_local_playlist("Локальный плейлист", &[])
        .unwrap();
    for (slot, content) in [
        ("album:7", "cache"),
        ("playlist:owner:3", "cache"),
        ("liked", "keep"),
    ] {
        assert!(
            store
                .save_music_snapshot(provider, None, region, slot, &content)
                .unwrap()
        );
    }

    let mut context = TestAppContext::single();
    let window = context.add_window(|window, cx| {
        crate::ui::theme::install(cx, false);
        MainView::new(window, cx, false, Some(true), false, Appearance::classic())
    });
    window
        .update(&mut context, |view, window, cx| {
            view.state.begin(crate::state::Intent::Startup);
            view.show_settings_for_capture(window, cx);
            assert!(view.is_settings_for_capture());
            request(view, cx);
            assert!(matches!(cx.global::<Operation>().phase, Phase::Clearing));
        })
        .unwrap();
    context.run_until_parked();
    window
        .update(&mut context, |view, _, cx| {
            assert!(view.is_settings_for_capture());
            assert!(matches!(cx.global::<Operation>().phase, Phase::Cleared));
        })
        .unwrap();
    assert!(
        store
            .music_snapshot::<String>(provider, None, region, "album:7")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .music_snapshot::<String>(provider, None, region, "playlist:owner:3")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .music_snapshot::<String>(provider, None, region, "liked")
            .unwrap()
            .as_deref(),
        Some("keep")
    );
    assert!(store.local_playlist(playlist).unwrap().is_some());
}
