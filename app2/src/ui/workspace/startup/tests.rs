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
    music::ObjectKind,
    providers::{Capabilities, Credentials, Profile, ProviderId, Session},
    state::{Intent, Screen},
    telemetry,
    ui::{
        CaptureLayout, application, open_capture_window,
        workspace::{Page, Target, pins::pin_of_target},
    },
};

use super::{MainView, Route, Store};

mod queue_resume;

#[test]
#[ignore = "uses a fresh MEDIADEKA_PROFILE_DIR and offscreen Windows D3D11"]
fn performance_startup_search_and_pins() {
    let profile = PathBuf::from(env::var_os("MEDIADEKA_PROFILE_DIR").unwrap());
    assert!(!profile.join("mediadeka.sqlite3").exists());
    let evidence = PathBuf::from(env::var_os("MEDIADEKA_WORKSPACE_EVIDENCE").unwrap());
    fs::create_dir_all(&evidence).unwrap();
    let store = Store::open(&profile).unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let provider = ProviderId::Jellyfin;
    let id = format!("http://{address}/#performance");
    store
        .use_account(
            &Session {
                provider,
                profile: Profile {
                    id: id.clone(),
                    name: "Performance fixture".into(),
                },
                capabilities: Capabilities {
                    home: true,
                    profile_selection: false,
                    device_code: false,
                },
                credentials: Credentials::Jellyfin {
                    server: format!("http://{address}"),
                    device: "startup-performance".into(),
                    access_token: "fixture".into(),
                },
            },
            || Some(()),
            || {},
        )
        .unwrap();
    store
        .save_workspace_route(&serde_json::to_string(&Route::History).unwrap())
        .unwrap();
    for index in 0..128 {
        let title = format!("Fixture playlist {index:03}");
        let playlist = store.create_local_playlist(&title, &[]).unwrap();
        store
            .pin_object(&pin_of_target(&Target::local(playlist, title)).into_storage())
            .unwrap();
    }

    let outcome = Arc::new(Mutex::new(None));
    let result = Arc::clone(&outcome);
    let check_store = store.clone();
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let checked = capture(cx, &check_store, &evidence).await;
            *result.lock().unwrap() = Some(checked);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    drop(listener);
    store
        .disconnect_account(provider, &id, || Some(()))
        .unwrap();
    outcome.lock().unwrap().take().unwrap().unwrap();
}

async fn capture(cx: &mut AsyncApp, store: &Store, evidence: &Path) -> gpui::Result<()> {
    let began = Instant::now();
    let initial_requests = telemetry::received().api_requests;
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
    window.update(cx, |view, window, cx| {
        view.start_desktop(began, window, cx);
        view.load_pins(cx);
    })?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if window.update(cx, |view, _, _| {
            view.music_page() == Page::History && view.has_pins()
        })? {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the initial route or pins did not load"
        );
        cx.background_executor()
            .timer(Duration::from_millis(5))
            .await;
    }
    let elapsed = window.update(cx, |view, window, cx| {
        assert!(matches!(
            view.state.screen(),
            Screen::Pending(Intent::Startup)
        ));
        view.focus_global_search(window, cx);
        assert!(view.workspace.search.is_focused(window, cx));
        view.startup_first_action().unwrap()
    })?;
    let history = frame(window, cx, evidence, "restored-history")?;
    assert!(history.contains("history-title"));
    let pins = history.match_indices("home-pinned-item").count();
    assert!(history.contains("Fixture playlist 000") && !history.contains("Fixture playlist 127"));

    window.update(cx, |view, window, cx| {
        view.submit_catalog(
            ObjectKind::Playlist,
            "Fixture playlist 000".into(),
            window,
            cx,
        );
        view.submit_catalog(
            ObjectKind::Playlist,
            "Fixture playlist 127".into(),
            window,
            cx,
        );
    })?;
    loop {
        let search = frame(window, cx, evidence, "local-search")?;
        if search.contains("music-search-saved") && search.contains("Fixture playlist 127") {
            assert!(!search.contains("music-catalog-result-jellyfin"));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the saved search result did not render"
        );
        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    }
    loop {
        if store
            .workspace_route()?
            .is_some_and(|payload| payload.contains("Fixture playlist 127"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the accepted search route was not persisted"
        );
        cx.background_executor()
            .timer(Duration::from_millis(5))
            .await;
    }
    let report = format!(
        "first accepted workspace action: {:.2} ms; visible pins: {pins}/128; HTTP requests during restore and two rapid search submissions: {}\n",
        elapsed.as_secs_f64() * 1000.,
        telemetry::received()
            .api_requests
            .saturating_sub(initial_requests),
    );
    print!("{report}");
    fs::write(evidence.join("startup.txt"), report)?;
    Ok(())
}

fn frame(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    evidence: &Path,
    name: &str,
) -> gpui::Result<String> {
    AnyWindowHandle::from(window).update(cx, |_, window, cx| {
        let handle = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(io::Error::other("expected a Win32 capture window").into());
        };
        const UIA_ROOT: isize = -25;
        unsafe { SendMessageW(handle.hwnd.get() as _, WM_GETOBJECT, 0, UIA_ROOT) };
        window.draw(cx).clear(cx);
        let tree = window
            .debug_a11y_tree_json()
            .ok_or_else(|| io::Error::other("missing accessibility tree"))?;
        window
            .render_to_image()?
            .save(evidence.join(format!("{name}.png")))?;
        fs::write(evidence.join(format!("{name}-a11y.json")), &tree)?;
        Ok(tree)
    })?
}
