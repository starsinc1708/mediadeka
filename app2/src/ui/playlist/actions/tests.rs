use std::{env, process::Command};

use gpui::TestAppContext;
use rusqlite::{Connection, TransactionBehavior};

use crate::{appearance::Appearance, storage::Store};

use super::*;

#[cfg(windows)]
mod import;
#[cfg(windows)]
mod local;

#[test]
fn returning_to_the_library_during_rename_shows_the_committed_name() {
    const FIXTURE_PROCESS: &str = "MEDIADEKA_TEST_LOCAL_RENAME_FIXTURE";
    if env::var_os(FIXTURE_PROCESS).is_none() {
        let profile = tempfile::tempdir().unwrap();
        let test_name = concat!(
            module_path!(),
            "::returning_to_the_library_during_rename_shows_the_committed_name"
        );
        let output = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                test_name.split_once("::").unwrap().1,
                "--nocapture",
            ])
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
        return;
    }

    let profile = Store::profile_directory().unwrap();
    let store = Store::open(&profile).unwrap();
    let id = store.create_local_playlist("Before", &[]).unwrap();
    let other_id = store.create_local_playlist("Unchanged", &[]).unwrap();
    let mut connection = Connection::open(profile.join("mediadeka.sqlite3")).unwrap();
    let mut context = TestAppContext::single();
    let window = context.add_window(|window, cx| {
        crate::ui::theme::install(cx, false);
        MainView::new(window, cx, false, Some(true), false, Appearance::classic())
    });
    window
        .update(&mut context, |view, window, cx| {
            view.open_local_playlist_for_capture(id, window, cx);
        })
        .unwrap();
    context.run_until_parked();

    let writer = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    window
        .update(&mut context, |view, window, cx| {
            assert_eq!(view.playlist_status_for_capture(), Some(Ok(0)));
            let Route::Playlist(target) = view.workspace.routes.current() else {
                panic!("Expected an open playlist");
            };
            let persisted_route_name = target.name.clone();
            view.edit_local_playlist(Edit::Rename(id), window, cx);
            view.playlist.editor.update(cx, |field, cx| {
                field.set_text("After", cx);
            });
            view.submit_local_change(cx);
            assert!(view.playlist.local_busy);
            assert_eq!(view.playlist.local_name(id), Some("After"));
            assert!(matches!(
                view.workspace.routes.current(),
                Route::Playlist(target) if target.name == persisted_route_name
            ));
        })
        .unwrap();
    window
        .update(&mut context, |view, window, cx| {
            view.playlist_history_for_capture(false, window, cx);
            assert!(view.music_page() == Page::Playlists);
            assert!(view.playlist.local_busy);
            assert!(view.playlist.open.is_none());
            assert!(
                view.playlist
                    .local
                    .iter()
                    .any(|row| row.id == id && row.name == "After")
            );
        })
        .unwrap();
    assert_eq!(store.local_playlist(id).unwrap().unwrap().name, "Before");
    writer.rollback().unwrap();
    context.run_until_parked();

    assert_eq!(store.local_playlist(id).unwrap().unwrap().name, "After");
    window
        .update(&mut context, |view, _, _| {
            assert!(view.music_page() == Page::Playlists);
            assert!(!view.playlist.local_busy);
            assert_eq!(
                view.playlist
                    .local
                    .iter()
                    .find(|row| row.id == id)
                    .unwrap()
                    .name,
                "After",
            );
            assert!(
                view.playlist
                    .local
                    .iter()
                    .any(|row| row.id == other_id && row.name == "Unchanged")
            );
        })
        .unwrap();

    window
        .update(&mut context, |view, window, cx| {
            view.playlist_history_for_capture(true, window, cx);
        })
        .unwrap();
    context.run_until_parked();
    window
        .update(&mut context, |view, window, cx| {
            view.edit_local_playlist(Edit::Rename(id), window, cx);
            view.playlist.editor.update(cx, |field, cx| {
                field.set_text(" ", cx);
            });
            view.submit_local_change(cx);
            view.playlist_history_for_capture(false, window, cx);
        })
        .unwrap();
    context.run_until_parked();
    assert_eq!(store.local_playlist(id).unwrap().unwrap().name, "After");
    window
        .update(&mut context, |view, _, _| {
            assert!(view.playlist.notice.is_some());
            assert!(
                view.playlist
                    .local
                    .iter()
                    .any(|row| row.id == id && row.name == "After")
            );
        })
        .unwrap();

    connection
        .execute_batch(
            "CREATE TRIGGER simulated_write_failure BEFORE UPDATE ON local_playlist
          WHEN new.name = 'Fail'
          BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;
         CREATE TRIGGER simulated_delete_failure BEFORE DELETE ON local_playlist
          BEGIN SELECT RAISE(ABORT, 'fixture delete failure'); END;",
        )
        .unwrap();
    window
        .update(&mut context, |view, _, cx| {
            view.start_local_change(
                LocalChange::Rename {
                    id,
                    name: "Fail".into(),
                },
                cx,
            );
            assert_eq!(view.playlist.local_name(id), Some("Fail"));
            assert!(view.playlist.local_busy);
        })
        .unwrap();
    context.run_until_parked();
    window
        .update(&mut context, |view, _, cx| {
            assert!(view.playlist.notice.is_some());
            assert_eq!(view.playlist.local_name(id), Some("After"));
            view.start_local_change(LocalChange::Delete(id), cx);
            assert!(view.playlist.local.iter().all(|row| row.id != id));
            assert!(view.playlist.local_busy);
        })
        .unwrap();
    context.run_until_parked();
    window
        .update(&mut context, |view, _, _| {
            assert!(view.playlist.notice.is_some());
            assert_eq!(view.playlist.local_name(id), Some("After"));
        })
        .unwrap();
    assert_eq!(store.local_playlist(id).unwrap().unwrap().name, "After");

    let writer = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    window
        .update(&mut context, |view, _, cx| {
            view.start_local_change(
                LocalChange::Create {
                    name: " New ".into(),
                    files: Vec::new(),
                },
                cx,
            );
            assert!(view.playlist.local_busy);
            assert!(
                view.playlist
                    .local
                    .iter()
                    .all(|playlist| playlist.name != "New")
            );
            assert!(matches!(
                &view.playlist.pending_local_edit,
                Some(crate::ui::playlist::local::PendingEdit::Create { name }) if name == "New"
            ));
        })
        .unwrap();
    assert_eq!(store.local_playlists().unwrap().len(), 2);
    writer.rollback().unwrap();
    context.run_until_parked();
    let created = store
        .local_playlists()
        .unwrap()
        .into_iter()
        .find(|playlist| playlist.name == "New")
        .unwrap();
    assert!(created.id > 0);
    window.update(&mut context, |view, _, _| {
        assert!(view.playlist.pending_local_edit.is_none());
        assert!(matches!(
            view.workspace.routes.current(),
            Route::Playlist(target) if target.key == PlaylistKey::Local(created.id) && target.name == "New"
        ));
    }).unwrap();
}
