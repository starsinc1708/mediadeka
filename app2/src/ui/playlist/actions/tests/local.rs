use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, KeyUpEvent, Keystroke, TestAppContext, WindowHandle,
};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::Value;

use crate::{
    appearance::Appearance,
    playback::Status,
    storage::{ImportProgress, Store},
    ui::{CaptureLayout, application, open_capture_window},
};

use super::{
    Edit, LocalChange, MainView,
    import::{frame, wait},
};

#[test]
#[ignore = "renders the local playlist controls through GPUI/D3D11 in a new isolated profile"]
fn local_playlist_actions_work_at_both_window_sizes() {
    let profile = Store::profile_directory().unwrap();
    let evidence = PathBuf::from(env::var_os("MEDIADEKA_PLAYLIST_EVIDENCE").unwrap());
    let audio = PathBuf::from(env::var_os("MEDIADEKA_IMPORT_AUDIO").unwrap())
        .canonicalize()
        .unwrap();
    assert!(!profile.join("mediadeka.sqlite3").exists());
    let store = Store::open(&profile).unwrap();
    store.use_guest(|| Some(())).unwrap();

    let capture_store = store.clone();
    let capture_audio = audio.clone();
    let result = Arc::new(Mutex::new(None));
    let outcome = result.clone();
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let captured: gpui::Result<()> = async {
                for (width, height) in [(1920., 1080.), (800., 400.)] {
                    let directory = evidence.join(format!("{width}x{height}"));
                    fs::create_dir_all(&directory)?;
                    let window = cx.update(|cx| {
                        open_capture_window(
                            cx,
                            CaptureLayout {
                                width,
                                height,
                                reduced_motion: true,
                            },
                        )
                    })?;
                    check_local_controls(
                        window,
                        cx,
                        &directory,
                        &capture_store,
                        &capture_audio,
                        width,
                        height,
                    )
                    .await?;
                    check_append_projection(
                        window,
                        cx,
                        &directory,
                        &capture_store,
                        &capture_audio,
                        width,
                    )
                    .await?;
                }
                Ok(())
            }
            .await;
            *outcome.lock().unwrap() = Some(captured);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    result.lock().unwrap().take().unwrap().unwrap();

    // Run this after the native frames so their text system registers the icon font first.
    let path_test_id = store.create_local_playlist("Выбор файла", &[]).unwrap();
    check_file_prompt(&store, path_test_id, &audio);
    assert!(store.delete_local_playlist(path_test_id).unwrap());
}

fn check_file_prompt(store: &Store, id: i64, audio: &Path) {
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
    for paths in [None, Some(vec![audio.to_owned()])] {
        let count = usize::from(paths.is_some());
        window
            .update(&mut context, |view, _, cx| {
                view.add_files_to_playlist(id, cx);
            })
            .unwrap();
        assert!(context.did_prompt_for_paths());
        context.simulate_path_prompt_response(|options| {
            assert!(options.files && options.multiple && !options.directories);
            paths
        });
        context.run_until_parked();
        assert_eq!(store.local_playlists().unwrap().len(), 1);
        assert_eq!(
            store.local_playlist(id).unwrap().unwrap().tracks.len(),
            count
        );
        window
            .update(&mut context, |view, _, _| {
                assert_eq!(view.playlist_status_for_capture(), Some(Ok(count)));
            })
            .unwrap();
    }
}

async fn check_append_projection(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    evidence: &Path,
    store: &Store,
    audio: &Path,
    width: f32,
) -> gpui::Result<()> {
    let other = store.directory().join(format!("other-{width}.wav"));
    fs::copy(audio, &other)?;
    let both = [audio.to_owned(), other.clone()];
    let prepared = store.create_local_playlist("Prepare tags for the held writer", &both)?;
    let files = [both[0].clone(), other.clone(), both[0].clone()];
    let prepared_rows = store.local_playlist(prepared)?.unwrap().tracks;
    assert!(store.delete_local_playlist(prepared)?);
    let expected_titles = [
        prepared_rows[0].title.as_str(),
        prepared_rows[1].title.as_str(),
        prepared_rows[0].title.as_str(),
    ]
    .join(" · ");
    let id = store.create_local_playlist("Append proof", &both[..1])?;
    let original = store.local_playlist(id)?.unwrap();
    let open =
        |view: &mut MainView, window: &mut gpui::Window, cx: &mut gpui::Context<MainView>| {
            view.open_local_playlist_for_capture(id, window, cx);
        };
    window.update(cx, open)?;
    wait(window, cx, |view| {
        view.playlist_status_for_capture() == Some(Ok(1))
    })
    .await?;
    window.update(cx, |view, window, cx| view.select_track(0, window, cx))?;
    let mut writer = Connection::open(store.directory().join("mediadeka.sqlite3"))?;
    writer.execute_batch(
        "CREATE TRIGGER simulated_track_append_error BEFORE INSERT ON local_playlist_track
         BEGIN SELECT RAISE(ABORT, 'fixture append error'); END;",
    )?;
    let locked = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let start = |view: &mut MainView, _: &mut gpui::Window, cx: &mut gpui::Context<MainView>| {
        view.start_local_change(
            LocalChange::Append {
                id,
                files: files.to_vec(),
            },
            cx,
        );
    };
    window.update(cx, start)?;
    wait(window, cx, |view| {
        matches!(
            view.playlist.import.as_ref().map(|import| &import.progress),
            Some(ImportProgress::Appending { count: 3, .. })
        )
    })
    .await?;
    let pending: Value = serde_json::from_str(&frame(window, cx, evidence, "append-pending")?)?;
    assert_eq!(
        node(&pending, "playlist-import-progress")["aria"]["label"],
        "Добавляем треки · 3…"
    );
    assert_eq!(
        node(&pending, "playlist-import-preview")["aria"]["label"],
        expected_titles
    );
    assert_eq!(store.local_playlist(id)?.unwrap(), original);
    locked.rollback()?;
    wait(window, cx, |view| {
        !view.playlist.local_busy && view.playlist.notice.is_some()
    })
    .await?;
    let aborted: Value = serde_json::from_str(&frame(window, cx, evidence, "append-rolled-back")?)?;
    assert!(aborted.to_string().contains("fixture append error"));
    assert!(node(&aborted, "playlist-import-progress").is_null());
    assert_eq!(store.local_playlist(id)?.unwrap(), original);
    writer.execute_batch("DROP TRIGGER simulated_track_append_error")?;

    window.update(cx, start)?;
    wait(window, cx, |view| {
        view.playlist_status_for_capture() == Some(Ok(4))
    })
    .await?;
    let rows = store.local_playlist(id)?.unwrap().tracks;
    assert_eq!(
        rows.iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        [
            prepared_rows[0].id.as_str(),
            prepared_rows[0].id.as_str(),
            prepared_rows[1].id.as_str(),
            prepared_rows[0].id.as_str()
        ]
    );
    window.update(cx, |view, _, _| {
        let open = view.playlist.open.as_ref().unwrap();
        assert_eq!(open.selected, Some(0));
        assert_eq!(open.playlist().unwrap().tracks, rows);
    })?;
    let committed: Value = serde_json::from_str(&frame(window, cx, evidence, "append-committed")?)?;
    assert!(node(&committed, "playlist-import-progress").is_null());
    assert!(store.delete_local_playlist(id)?);
    Ok(())
}

async fn check_local_controls(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    evidence: &Path,
    store: &Store,
    audio: &Path,
    width: f32,
    height: f32,
) -> gpui::Result<()> {
    const NAME: &str = "Ночной кодинг и музыка для длинного маршрута домой по пустым улицам города";
    let id = store.create_local_playlist(NAME, &[])?;
    window.update(cx, |view, window, cx| {
        view.open_local_playlist_for_capture(id, window, cx)
    })?;
    wait(window, cx, |view| {
        view.playlist_status_for_capture() == Some(Ok(0))
    })
    .await?;
    let empty: Value = serde_json::from_str(&frame(window, cx, evidence, "empty")?)?;
    assert_eq!(empty["frame"]["viewport_size"]["width"], width);
    assert_eq!(empty["frame"]["viewport_size"]["height"], height);
    assert_eq!(
        node(&empty, "playlist-add-files")["aria"]["label"],
        "Добавить файлы"
    );
    for id in [
        "playlist-play",
        "playlist-shuffle",
        "playlist-sort",
        "playlist-search",
        "playlist-columns",
    ] {
        assert!(
            node(&empty, id).is_null(),
            "{id} should be absent on an empty playlist"
        );
    }

    window.update(cx, |view, window, cx| {
        view.edit_local_playlist(Edit::Rename(id), window, cx)
    })?;
    let rename: Value = serde_json::from_str(&frame(window, cx, evidence, "rename")?)?;
    assert_eq!(node(&rename, "playlist-name")["aria"]["value"], NAME);
    assert_eq!(focused_element(&rename), r#"Name("playlist-name")"#);
    assert_eq!(
        node(&rename, "playlist-confirm-edit")["aria"]["label"],
        "Сохранить название"
    );
    assert_eq!(
        node(&rename, "playlist-cancel-edit")["aria"]["label"],
        "Отмена"
    );

    keystroke(window, cx, "escape")?;
    const RENAMED: &str = "Подтверждённое название";
    let mut connection = Connection::open(Store::profile_directory()?.join("mediadeka.sqlite3"))?;
    let writer = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    window.update(cx, |view, window, cx| {
        view.edit_local_playlist(Edit::Rename(id), window, cx);
        view.playlist
            .editor
            .update(cx, |field, cx| field.set_text(RENAMED, cx));
        view.submit_local_change(cx);
        assert_eq!(view.playlist.local_name(id), Some(RENAMED));
    })?;
    let pending: Value = serde_json::from_str(&frame(window, cx, evidence, "rename-pending")?)?;
    assert_eq!(
        node(&pending, "playlist-confirm-edit")["aria"]["label"],
        "Сохраняем…"
    );
    assert_eq!(store.local_playlist(id)?.unwrap().name, NAME);
    writer.rollback()?;
    wait(window, cx, |view| {
        !view.playlist.local_busy && view.playlist.local_name(id) == Some(RENAMED)
    })
    .await?;
    assert_eq!(store.local_playlist(id)?.unwrap().name, RENAMED);

    window.update(cx, |view, window, cx| {
        view.edit_local_playlist(Edit::Delete(id), window, cx)
    })?;
    let delete: Value = serde_json::from_str(&frame(window, cx, evidence, "delete")?)?;
    assert_eq!(
        node(&delete, "playlist-confirm-edit")["aria"]["label"],
        "Удалить плейлист"
    );
    assert_eq!(focused_element(&delete), r#"Name("playlist-cancel-edit")"#);
    keystroke(window, cx, "enter")?; // The initially focused button cancels deletion.

    window.update(cx, |view, _, cx| {
        assert!(view.playlist.edit.is_none());
        view.start_local_change(
            LocalChange::Append {
                id,
                files: vec![audio.to_owned()],
            },
            cx,
        );
    })?;
    wait(window, cx, |view| {
        view.playlist_status_for_capture() == Some(Ok(1))
    })
    .await?;
    let filled: Value = serde_json::from_str(&frame(window, cx, evidence, "with-track")?)?;
    assert_eq!(node(&filled, "playlist-play")["aria"]["label"], "Слушать");
    assert_eq!(!node(&filled, "playlist-columns").is_null(), width == 1920.);
    assert_eq!(store.local_playlists()?.len(), 1);
    assert_eq!(store.local_playlist(id)?.unwrap().tracks.len(), 1);

    window.update(cx, |view, _, cx| {
        view.play_playlist_track_for_capture(0, cx)
    })?;
    wait(window, cx, |view| {
        view.player_snapshot.status == Status::Playing
    })
    .await?;
    let previous_position = window.update(cx, |view, _, cx| view.player.read(cx).position())?;
    connection.execute_batch(
        "CREATE TRIGGER simulated_track_remove_error BEFORE DELETE ON local_playlist_track
         BEGIN SELECT RAISE(ABORT, 'fixture remove error'); END;",
    )?;
    choose_remove_track(window, cx, evidence, "remove-menu")?;
    wait(window, cx, |view| {
        !view.playlist.local_busy && view.playlist.notice.is_some()
    })
    .await?;
    let rolled_back: Value =
        serde_json::from_str(&frame(window, cx, evidence, "remove-rolled-back")?)?;
    assert!(rolled_back.to_string().contains("fixture remove error"));
    assert_eq!(store.local_playlist_snapshot(id)?.unwrap().revision, None);
    connection.execute_batch("DROP TRIGGER simulated_track_remove_error")?;

    let writer = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    choose_remove_track(window, cx, evidence, "remove-menu-confirmed")?;
    let pending: Value = serde_json::from_str(&frame(window, cx, evidence, "remove-pending")?)?;
    assert_eq!(
        node(&pending, "playlist-local-pending")["aria"]["label"],
        "Удаляем трек…"
    );
    assert_eq!(store.local_playlist(id)?.unwrap().tracks.len(), 1);
    writer.rollback()?;
    wait(window, cx, |view| {
        view.playlist_status_for_capture() == Some(Ok(0))
    })
    .await?;
    let persisted = store.local_playlist_snapshot(id)?.unwrap();
    assert_eq!(persisted.revision, Some(1));
    assert!(persisted.playlist.tracks.is_empty() && audio.is_file());
    window.update(cx, |view, _, cx| {
        assert_eq!(view.player_snapshot.status, Status::Playing);
        assert_eq!(view.player_snapshot.queue_len, 1);
        assert_eq!(view.player_snapshot.index, Some(0));
        assert!(view.player.read(cx).position() > previous_position);
    })?;
    let removed: Value = serde_json::from_str(&frame(window, cx, evidence, "remove-committed")?)?;
    assert!(removed.to_string().contains("В плейлисте пока нет треков."));
    window.update(cx, |view, _, cx| {
        view.player.update(cx, |bar, cx| {
            bar.command(cx, |playback| playback.toggle_pause())
        });
    })?;
    assert!(store.delete_local_playlist(id)?);
    Ok(())
}

fn choose_remove_track(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    evidence: &Path,
    stem: &str,
) -> gpui::Result<()> {
    window.update(cx, |view, window, cx| {
        view.show_playlist_menu_for_capture(0, window, cx)
    })?;
    let tree: Value = serde_json::from_str(&frame(window, cx, evidence, stem)?)?;
    let item = tree["nodes"]
        .as_object()
        .unwrap()
        .values()
        .find(|node| {
            node["aria"]["role"] == "MenuItem" && node["aria"]["label"] == "Убрать из плейлиста"
        })
        .expect("The local row menu offers removal");
    let id = item["accesskit_id"].as_str().unwrap();
    keystroke(window, cx, "up")?;
    let active: Value =
        serde_json::from_str(&frame(window, cx, evidence, &format!("{stem}-focused"))?)?;
    let selected = active["active_descendant_focus"].as_str().unwrap();
    assert_eq!(active["nodes"][selected]["accesskit_id"], id);
    keystroke(window, cx, "enter")
}

fn node<'a>(tree: &'a Value, id: &str) -> &'a Value {
    let id = format!(r#"Name("{id}")"#);
    tree["nodes"]
        .as_object()
        .unwrap()
        .values()
        .find(|node| node["element_id"] == id)
        .unwrap_or(&Value::Null)
}

fn focused_element(tree: &Value) -> &Value {
    &tree["nodes"][tree["gpui_focus"].as_str().unwrap_or_default()]["element_id"]
}

fn keystroke(window: WindowHandle<MainView>, cx: &mut AsyncApp, stroke: &str) -> gpui::Result<()> {
    let keystroke = Keystroke::parse(stroke)?;
    AnyWindowHandle::from(window).update(cx, |_, window, cx| {
        window.dispatch_keystroke(keystroke.clone(), cx);
        window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
        window.draw(cx).clear(cx);
    })
}
