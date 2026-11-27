use std::{
    env, fs,
    io::{self, BufRead, BufReader, Write},
    net::TcpListener,
    path::Path,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use gpui::{AnyWindowHandle, AsyncApp};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETOBJECT};

use crate::{
    music::{ObjectKind, ObjectRef, now, region},
    providers::{Capabilities, Credentials, Profile, ProviderId, Session, yandex::test_server},
    storage::{PanelTab, Store},
    telemetry,
    ui::{
        CaptureLayout, application, open_capture_window,
        workspace::{MusicTarget, Page},
    },
};

use super::*;

const CHILD: &str = "MEDIADEKA_CONTENT_TIMING_CHILD";
const EVIDENCE: &str = "MEDIADEKA_CONTENT_TIMING_EVIDENCE";
const TEST: &str =
    "ui::workspace::content_timing::tests::native_route_timing_waits_for_visible_catalog_data";
const PROFILE: &str = "74368";

#[test]
#[ignore = "uses native Windows D3D11 and the Yandex adapter against a delayed loopback response"]
fn native_route_timing_waits_for_visible_catalog_data() {
    if env::var_os(CHILD).is_none() {
        let profile = tempfile::tempdir().unwrap();
        let status = Command::new(env::current_exe().unwrap())
            .args([TEST, "--exact", "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("MEDIADEKA_PROFILE_DIR", profile.path())
            .env("MEDIADEKA_GUI", "3")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }

    let evidence = std::path::PathBuf::from(env::var_os(EVIDENCE).unwrap());
    fs::create_dir_all(&evidence).unwrap();
    let store = Store::open_default().unwrap();
    let provider = ProviderId::YandexMusic;
    let target = MusicTarget {
        object: ObjectRef {
            provider,
            kind: ObjectKind::Album,
            id: "42".into(),
        },
        profile: Some(PROFILE.into()),
        title: "Название до загрузки".into(),
        artwork: None,
    };
    store
        .connect_account(
            &Session {
                provider,
                profile: Profile {
                    id: PROFILE.into(),
                    name: "Content timing fixture".into(),
                },
                credentials: Credentials::YandexMusic {
                    access_token: "fixture-content-timing".into(),
                    refresh_token: None,
                    access_expires_at: None,
                },
                capabilities: Capabilities {
                    home: true,
                    profile_selection: false,
                    device_code: true,
                },
            },
            || Some(()),
            || {},
        )
        .unwrap()
        .unwrap();
    let started = now();
    assert!(
        store
            .claim_music_match(&region(), &target.object, started, started + 3_600)
            .unwrap()
    );
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    test_server::use_loopback(listener.local_addr().unwrap());
    let (release, response) = mpsc::channel();
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&stopped);
    let server = thread::spawn(move || serve(listener, response, &stop));
    let result = Arc::new(Mutex::new(None));
    let completion = Arc::clone(&result);
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            *completion.lock().unwrap() = Some(check(cx, &target, &evidence, release).await);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    stopped.store(true, Ordering::Relaxed);
    let album_requests = server.join().unwrap();
    let _ = store.disconnect_account(provider, PROFILE, || Some(()));
    result.lock().unwrap().take().unwrap().unwrap();
    assert_eq!(album_requests, 1);
    println!("HTTP requests for album 42: {album_requests}");
}

fn serve(listener: TcpListener, release: mpsc::Receiver<()>, stopped: &AtomicBool) -> u64 {
    let mut album_requests = 0;
    while !stopped.load(Ordering::Relaxed) {
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(4));
                continue;
            }
            Err(error) => panic!("{error}"),
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        let mut request = BufReader::new(&stream);
        let mut line = String::new();
        request.read_line(&mut line).unwrap();
        let album = line.starts_with("GET /albums/42/with-tracks HTTP/");
        loop {
            line.clear();
            if request.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                break;
            }
        }
        let (status, body) = if album {
            if album_requests == 0 {
                release.recv_timeout(Duration::from_secs(12)).unwrap();
            }
            album_requests += 1;
            (
                "200 OK",
                r#"{"result":{"id":42,"title":"Ответ каталога","artists":[{"id":71,"name":"Исполнитель из ответа"}]}}"#,
            )
        } else {
            ("404 Not Found", "{}")
        };
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    }
    album_requests
}

async fn check(
    cx: &mut AsyncApp,
    target: &MusicTarget,
    evidence: &Path,
    release: mpsc::Sender<()>,
) -> gpui::Result<()> {
    let window = cx.update(|cx| {
        open_capture_window(
            cx,
            CaptureLayout {
                width: 800.,
                height: 400.,
                reduced_motion: true,
            },
        )
    })?;
    let raw = AnyWindowHandle::from(window);
    window.update(cx, |view, window, cx| {
        view.start_home_skeleton_for_capture(window, cx)
    })?;
    frame(raw, cx, evidence, "workspace-primed")?;
    let before = telemetry::received().api_requests;
    window.update(cx, |view, window, cx| {
        view.navigate(Route::Object(target.clone()), window, cx)
    })?;
    let heading = frame(raw, cx, evidence, "seed-before-response")?;
    assert!(heading.contains("music-object-loading"));
    assert!(!heading.contains("Ответ каталога"));
    assert!(window.update(cx, |view, _, _| view.last_content_opening().is_none())?);
    cx.background_executor()
        .timer(Duration::from_millis(150))
        .await;
    release.send(())?;

    let wait_for_data = async |cx: &mut AsyncApp| -> gpui::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(6);
        while !window.update(cx, |view, _, _| view.has_loaded_catalog_object(target))? {
            assert!(Instant::now() < deadline);
            cx.background_executor()
                .timer(Duration::from_millis(4))
                .await;
        }
        Ok(())
    };
    wait_for_data(cx).await?;
    assert!(frame(raw, cx, evidence, "catalog-cold")?.contains("Ответ каталога"));
    let cold = window.update(cx, |view, _, _| view.last_content_opening().unwrap())?;
    assert_eq!(cold.kind, ContentKind::Catalog);

    window.update(cx, |view, window, cx| {
        view.show_music_page(Page::History, window, cx);
        view.navigate(Route::Object(target.clone()), window, cx);
    })?;
    wait_for_data(cx).await?;
    assert!(frame(raw, cx, evidence, "catalog-warm")?.contains("Ответ каталога"));
    let warm = window.update(cx, |view, _, _| view.last_content_opening().unwrap())?;
    assert!(warm.elapsed < cold.elapsed);

    window.update(cx, |view, window, cx| {
        view.show_music_page(Page::History, window, cx);
        view.navigate(Route::Object(target.clone()), window, cx);
        view.open_panel(PanelTab::Queue, false, window, cx);
        assert!(view.panel_replaces_page(window));
    })?;
    wait_for_data(cx).await?;
    assert!(!frame(raw, cx, evidence, "loaded-behind-panel")?.contains("Ответ каталога"));
    assert_eq!(
        window.update(cx, |view, _, _| view.last_content_opening())?,
        Some(warm)
    );
    window.update(cx, |view, window, cx| assert!(view.close_panel(window, cx)))?;
    assert!(frame(raw, cx, evidence, "loaded-after-panel")?.contains("Ответ каталога"));
    let revealed = window.update(cx, |view, _, _| view.last_content_opening().unwrap())?;
    let diag = window.update(cx, |view, window, cx| {
        view.open_sound_diagnostics_for_capture(window, cx)
    })??;
    cx.background_executor()
        .timer(Duration::from_millis(600))
        .await;
    let diag_tree = frame(diag, cx, evidence, "diagnostics-first-content")?;
    assert!(
        diag_tree.contains("diagnostics-content-opening")
            && diag_tree.contains("Открытие объекта каталога")
    );
    assert_eq!(
        window.update(cx, |view, _, _| view.last_content_opening())?,
        Some(revealed)
    );
    let report = format!(
        "cold {:.2} ms; warm {:.2} ms; revealed after full compact panel {:.2} ms; total HTTP requests across routes {}\n",
        cold.elapsed.as_secs_f64() * 1000.,
        warm.elapsed.as_secs_f64() * 1000.,
        revealed.elapsed.as_secs_f64() * 1000.,
        telemetry::received().api_requests - before,
    );
    fs::write(evidence.join("timing.txt"), &report)?;
    print!("{report}");
    Ok(())
}

fn frame(
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
    evidence: &Path,
    name: &str,
) -> gpui::Result<String> {
    let (tree, image) = window.update(cx, |_, window, cx| -> gpui::Result<_> {
        let handle = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(io::Error::other("expected Win32").into());
        };
        unsafe { SendMessageW(handle.hwnd.get() as _, WM_GETOBJECT, 0, -25) };
        window.draw(cx).clear(cx);
        let tree = window
            .debug_a11y_tree_json()
            .ok_or_else(|| io::Error::other("missing accessibility tree"))?;
        Ok((tree, window.render_to_image()?))
    })??;
    image.save(evidence.join(format!("{name}.png")))?;
    fs::write(evidence.join(format!("{name}-a11y.json")), &tree)?;
    Ok(tree)
}
