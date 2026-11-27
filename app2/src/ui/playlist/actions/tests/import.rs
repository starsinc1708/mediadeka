use std::{
    env, fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use rusqlite::{Connection, TransactionBehavior};
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETOBJECT};

use crate::{
    state::Intent,
    storage::Store,
    ui::{CaptureLayout, application, open_capture_window},
};

use super::{ImportProgress, LocalChange, MainView, Page};

#[test]
#[ignore = "renders the folder import through GPUI/D3D11 in a new isolated profile"]
fn folder_import_shows_live_progress_and_cancels() {
    let profile = Store::profile_directory().unwrap();
    let evidence = PathBuf::from(env::var_os("MEDIADEKA_IMPORT_EVIDENCE").unwrap());
    let audio = PathBuf::from(env::var_os("MEDIADEKA_IMPORT_AUDIO").unwrap());
    assert!(!profile.join("mediadeka.sqlite3").exists());
    fs::create_dir_all(&evidence).unwrap();
    let store = Store::open(&profile).unwrap();
    store.use_guest(|| Some(())).unwrap();
    let folder = tempfile::tempdir().unwrap();
    let extension = audio.extension().unwrap().to_str().unwrap();
    for index in 0..65 {
        fs::copy(
            &audio,
            folder.path().join(format!("{index:02}.{extension}")),
        )
        .unwrap();
    }

    let result = Arc::new(Mutex::new(None));
    let outcome = result.clone();
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let captured: gpui::Result<()> = async {
                let window = cx.update(|cx| {
                    open_capture_window(
                        cx,
                        CaptureLayout {
                            width: 1200.,
                            height: 720.,
                            reduced_motion: true,
                        },
                    )
                })?;
                let mut connection = Connection::open(profile.join("mediadeka.sqlite3"))?;
                let writer =
                    connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                window.update(cx, |view, window, cx| {
                    view.state.begin(Intent::Startup);
                    view.show_music_page(Page::Playlists, window, cx);
                    view.start_local_change(
                        LocalChange::Create {
                            name: "Импорт тестовой папки".into(),
                            files: vec![folder.path().to_owned()],
                        },
                        cx,
                    );
                })?;
                wait(window, cx, |view| {
                    matches!(
                        view.playlist.import.as_ref().map(|import| &import.progress),
                        Some(ImportProgress::Discovering { found: 64 })
                    )
                })
                .await?;
                let tree = frame(window, cx, &evidence, "folder-import")?;
                assert!(
                    tree.contains("playlist-cancel-import") && tree.contains("Найдено файлов: 64")
                );
                window.update(cx, |view, _, cx| view.cancel_playlist_import(cx))?;
                writer.rollback()?;
                wait(window, cx, |view| !view.playlist.local_busy).await?;
                assert!(store.local_playlists()?.is_empty());
                window.update(cx, |view, _, _| {
                    assert_eq!(view.playlist.notice.as_deref(), Some("Импорт отменён."));
                })?;
                let tree = frame(window, cx, &evidence, "folder-import-cancelled")?;
                assert!(
                    tree.contains("Импорт отменён.") && !tree.contains("playlist-cancel-import")
                );
                Ok(())
            }
            .await;
            *outcome.lock().unwrap() = Some(captured);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    result.lock().unwrap().take().unwrap().unwrap();
}

pub(super) fn frame(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    evidence: &Path,
    stem: &str,
) -> gpui::Result<String> {
    AnyWindowHandle::from(window).update(cx, |_, window, cx| {
        let handle = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(io::Error::other("Expected a Win32 window").into());
        };
        const UIA_ROOT_OBJECT_ID: isize = -25;
        unsafe { SendMessageW(handle.hwnd.get() as _, WM_GETOBJECT, 0, UIA_ROOT_OBJECT_ID) };
        window.draw(cx).clear(cx);
        window
            .render_to_image()?
            .save(evidence.join(format!("{stem}.png")))?;
        let tree = window
            .debug_a11y_tree_json()
            .ok_or_else(|| io::Error::other("Missing GPUI accessibility tree"))?;
        fs::write(evidence.join(format!("{stem}-a11y.json")), &tree)?;
        Ok(tree)
    })?
}

pub(super) async fn wait(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    ready: impl Fn(&MainView) -> bool,
) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !window.update(cx, |view, _, _| ready(view))? {
        if Instant::now() >= deadline {
            return Err(
                io::Error::other("The local playlist did not reach the expected state").into(),
            );
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    Ok(())
}
