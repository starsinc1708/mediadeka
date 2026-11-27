use std::{
    env, fs, io,
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETOBJECT};

use crate::{
    providers::{
        Capabilities, Credentials, ErrorKind, Home, HomeItem, HomeSection, Profile, ProviderError,
        Session,
    },
    state::{ActiveSession, HomeContent, HomeView, Issue},
    storage::{CacheScope, Snapshot, Startup},
    ui::{CaptureLayout, application, open_capture_window},
};

use super::*;

#[test]
#[ignore = "renders the two recovery actions through GPUI/D3D11 without desktop input"]
fn connection_recovery_frames() {
    let evidence = PathBuf::from(env::var_os("MEDIADEKA_CONNECTION_RECOVERY_EVIDENCE").unwrap());
    fs::create_dir_all(&evidence).unwrap();
    let outcome = Arc::new(Mutex::new(None));
    let result = Arc::clone(&outcome);
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let rendered = render_connection_recovery(cx, &evidence).await;
            *result.lock().unwrap() = Some(rendered);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    outcome.lock().unwrap().take().unwrap().unwrap();
}

async fn render_connection_recovery(cx: &mut AsyncApp, evidence: &Path) -> gpui::Result<()> {
    let provider = ProviderId::Jellyfin;
    let profile = Profile {
        id: "recovery-fixture".into(),
        name: "Тестовый профиль".into(),
    };
    let capabilities = Capabilities {
        home: true,
        profile_selection: false,
        device_code: true,
    };
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
    let raw = AnyWindowHandle::from(window);
    raw.update(cx, |_, window, _| -> gpui::Result<()> {
        let handle = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(io::Error::other("Expected a Win32 window").into());
        };
        const UIA_ROOT_OBJECT_ID: isize = -25;
        unsafe { SendMessageW(handle.hwnd.get() as _, WM_GETOBJECT, 0, UIA_ROOT_OBJECT_ID) };
        Ok(())
    })??;
    for (kind, stem, message, action) in [
        (
            ErrorKind::Network,
            "retry",
            "Сервер недоступен.",
            "Повторить",
        ),
        (
            ErrorKind::Expired,
            "reconnect",
            "Подключение истекло.",
            "Войти снова",
        ),
    ] {
        window.update(cx, |view, _, cx| {
            let request = view.state.begin(Intent::Startup);
            view.state.finish(
                &request,
                Screen::Home(HomeView {
                    session: ActiveSession::Account {
                        provider,
                        profile: profile.clone(),
                        capabilities,
                    },
                    content: HomeContent::Cached {
                        snapshot: snapshot("Сохранённая подборка Jellyfin"),
                        issue: Issue::Provider(ProviderError {
                            kind,
                            message: message.into(),
                        }),
                    },
                    accounts: vec![crate::storage::AccountSummary {
                        provider,
                        profile: profile.clone(),
                    }],
                    switch_issue: None,
                }),
            );
            view.filter_active_home_for_capture(cx);
        })?;
        cx.background_executor()
            .timer(Duration::from_millis(80))
            .await;
        frame(window, cx, &evidence.join(format!("{stem}.png")))?;
        let tree = raw
            .update(cx, |_, window, _| window.debug_a11y_tree_json())?
            .ok_or_else(|| io::Error::other("Missing GPUI accessibility tree"))?;
        let nodes: serde_json::Value = serde_json::from_str(&tree)?;
        assert!(
            nodes["nodes"]
                .as_object()
                .is_some_and(|nodes| nodes.values().any(|node| {
                    node["element_id"] == "Name(\"home-retry\")" && node["aria"]["label"] == action
                })),
            "Expected {action} in the GPUI accessibility tree"
        );
        fs::write(evidence.join(format!("{stem}-a11y.json")), tree)?;
    }
    window.update(cx, |view, window, cx| {
        view.show_music_page(crate::ui::workspace::Page::Settings, window, cx);
        view.open_settings_connection(provider, window, cx);
        assert!(view.is_settings_login());
    })?;
    cx.background_executor()
        .timer(Duration::from_millis(80))
        .await;
    frame(window, cx, &evidence.join("reconnect-form.png"))?;
    Ok(())
}

#[test]
#[ignore = "requires Windows D3D11 and an empty MEDIADEKA_PROFILE_DIR for the capture process"]
fn source_switch_renders_the_cached_feed_while_jellyfin_is_waiting() {
    let directory = PathBuf::from(env::var_os("MEDIADEKA_PROFILE_DIR").unwrap());
    assert!(!directory.join("mediadeka.sqlite3").exists());
    let evidence = PathBuf::from(env::var_os("MEDIADEKA_SOURCE_SWITCH_EVIDENCE").unwrap());
    fs::create_dir_all(&evidence).unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let store = Store::open(directory).unwrap();
    let id = format!("http://{address}/#fixture");
    let session = Session {
        provider: ProviderId::Jellyfin,
        profile: Profile {
            id: id.clone(),
            name: "Fixture".into(),
        },
        capabilities: Capabilities {
            home: true,
            profile_selection: false,
            device_code: false,
        },
        credentials: Credentials::Jellyfin {
            server: format!("http://{address}"),
            device: "switch-preview-fixture".into(),
            access_token: "fixture".into(),
        },
    };
    store.use_account(&session, || Some(()), || {}).unwrap();
    store.use_guest(|| Some(())).unwrap();
    store
        .save_cache(
            CacheScope::Guest,
            &snapshot("Гостевая подборка"),
            || Some(()),
        )
        .unwrap();
    store
        .save_cache(
            CacheScope::Account(session.provider, &id),
            &snapshot("Сохранённая подборка Jellyfin"),
            || Some(()),
        )
        .unwrap();

    let outcome = Arc::new(Mutex::new(None));
    let result = Arc::clone(&outcome);
    let selected = id.clone();
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let check = check_switch(cx, listener, selected, &evidence).await;
            *result.lock().unwrap() = Some(check);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    let checked = outcome.lock().unwrap().take().unwrap();
    let route = store.startup();
    store
        .disconnect_account(session.provider, &id, || Some(()))
        .unwrap();
    checked.unwrap();
    assert!(matches!(route.unwrap(), Startup::Account(account) if account.profile.id == id));
}

fn snapshot(title: &str) -> Snapshot {
    Snapshot::received(Home {
        title: title.into(),
        items: vec![HomeItem {
            id: "fixture-item".into(),
            title: title.into(),
            subtitle: "Тестовая запись".into(),
            url: None,
            artwork: None,
        }],
        sections: vec![HomeSection {
            title: title.into(),
            len: 1,
            ..Default::default()
        }],
    })
}

async fn check_switch(
    cx: &mut AsyncApp,
    listener: TcpListener,
    id: String,
    evidence: &Path,
) -> gpui::Result<()> {
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
    let profile = Store::open_default()?;
    window.update(cx, |view, _, _| {
        let startup = view.state.begin(Intent::Startup);
        let preview = startup.home_preview(&profile).unwrap();
        view.state.finish(&startup, Screen::Home(preview));
    })?;
    cx.background_executor()
        .timer(Duration::from_millis(80))
        .await;
    frame(window, cx, &evidence.join("before.png"))?;

    let started = Instant::now();
    window.update(cx, |view, _, cx| {
        view.run_account(ProviderId::Jellyfin, id, cx)
    })?;
    let mut socket = None;
    let mut preview_at = None;
    loop {
        if socket.is_none() {
            match listener.accept() {
                Ok((stream, _)) => socket = Some(stream),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
        }
        if preview_at.is_none() && has_jellyfin_feed(window, cx, true)? {
            preview_at = Some(started.elapsed());
        }
        if socket.is_some() && preview_at.is_some() {
            break;
        }
        if started.elapsed() > Duration::from_secs(4) {
            return Err(io::Error::other(
                "Jellyfin cache did not render while the socket was held open",
            )
            .into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(15))
            .await;
    }
    frame(window, cx, &evidence.join("cached-while-waiting.png"))?;
    let rendered_at = started.elapsed();
    cx.background_executor()
        .timer(Duration::from_millis(700))
        .await;
    if !has_jellyfin_feed(window, cx, true)? {
        return Err(
            io::Error::other("Selected feed disappeared while waiting for Jellyfin").into(),
        );
    }
    let released_at = started.elapsed();
    drop(listener);
    drop(socket);

    while !has_jellyfin_feed(window, cx, false)? {
        if started.elapsed() > Duration::from_secs(10) {
            return Err(
                io::Error::other("Jellyfin cache disappeared after the connection failed").into(),
            );
        }
        cx.background_executor()
            .timer(Duration::from_millis(15))
            .await;
    }
    frame(window, cx, &evidence.join("cached-offline.png"))?;
    println!(
        "Selected source preview at {} ms, frame at {} ms, socket closed at {} ms, cache preserved on failure",
        preview_at.unwrap().as_millis(),
        rendered_at.as_millis(),
        released_at.as_millis()
    );
    Ok(())
}

fn has_jellyfin_feed(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    pending: bool,
) -> gpui::Result<bool> {
    window.update(cx, |view, _, _| matches!(view.state.home_view(), Some((home, busy)) if busy == pending
        && matches!(home.session, ActiveSession::Account { provider: ProviderId::Jellyfin, .. })
        && match (&home.content, pending) {
            (HomeContent::Fresh(snapshot), true) | (HomeContent::Cached { snapshot, .. }, false) => snapshot.home.title == "Сохранённая подборка Jellyfin",
            _ => false,
        }))
}

fn frame(window: WindowHandle<MainView>, cx: &mut AsyncApp, path: &Path) -> gpui::Result<()> {
    AnyWindowHandle::from(window).update(cx, |_, window, cx| {
        window.draw(cx).clear(cx);
        window.render_to_image()?.save(path)?;
        Ok(())
    })?
}
