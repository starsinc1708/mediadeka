use std::{
    env, fs,
    io::{self, BufRead, BufReader, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent, ScrollDelta, ScrollWheelEvent, WindowHandle, point, px,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{Value, json};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowRect, SendMessageW, WM_DPICHANGED, WM_GETOBJECT,
};

use crate::{
    music::library::EntryKey,
    providers::{Capabilities, Credentials, Profile, ProviderId, Session, yandex::test_server},
    state::{ActiveSession, HomeContent, HomeView, Intent, Screen},
    storage::Store,
    ui::{CaptureLayout, MainView, application, open_capture_window, workspace::Page},
};

const TEST: &str = "ui::music::collections::native_refresh::collection_refresh_keeps_native_rows_and_scroll_stable";
const CHILD: &str = "MEDIADEKA_COLLECTION_REFRESH_CHILD";
const EVIDENCE: &str = "MEDIADEKA_COLLECTION_REFRESH_EVIDENCE";
const MODE: &str = "MEDIADEKA_COLLECTION_REFRESH_LAYOUT";
const PROFILE: &str = "74636";
const ALBUMS: usize = 72;
const TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Clone, Copy)]
struct Layout {
    width: u32,
    height: u32,
    dpi: u32,
}

impl Layout {
    fn from_env() -> Self {
        match env::var(MODE).as_deref() {
            Ok("wide") => Self {
                width: 1920,
                height: 1080,
                dpi: 96,
            },
            Ok("150") => Self {
                width: 800,
                height: 400,
                dpi: 144,
            },
            _ => Self {
                width: 800,
                height: 400,
                dpi: 96,
            },
        }
    }

    fn physical_size(self) -> (u32, u32) {
        (self.width * self.dpi / 96, self.height * self.dpi / 96)
    }
}

#[derive(Default)]
struct Server {
    received: AtomicUsize,
    released: AtomicUsize,
    stopped: AtomicBool,
}

#[test]
#[ignore = "renders a native collection while the real Yandex adapter waits on loopback"]
fn collection_refresh_keeps_native_rows_and_scroll_stable() {
    if env::var_os(CHILD).is_none() {
        let profile = tempfile::tempdir().unwrap();
        assert!(
            Command::new(env::current_exe().unwrap())
                .args(["--exact", TEST, "--ignored", "--nocapture"])
                .env(CHILD, "1")
                .env("MEDIADEKA_PROFILE_DIR", profile.path())
                .env("MEDIADEKA_GUI", "3")
                .status()
                .unwrap()
                .success()
        );
        return;
    }

    let evidence = PathBuf::from(env::var_os(EVIDENCE).unwrap());
    let layout = Layout::from_env();
    fs::create_dir_all(&evidence).unwrap();
    let store = Store::open_default().unwrap();
    let provider = ProviderId::YandexMusic;
    let session = Session {
        provider,
        profile: Profile {
            id: PROFILE.into(),
            name: "Refresh fixture".into(),
        },
        credentials: Credentials::YandexMusic {
            access_token: "fixture-collection-refresh".into(),
            refresh_token: None,
            access_expires_at: None,
        },
        capabilities: Capabilities {
            home: true,
            profile_selection: false,
            device_code: true,
        },
    };
    store.connect_account(&session, || Some(()), || {}).unwrap();
    let accounts = store.accounts().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    test_server::use_loopback(listener.local_addr().unwrap());
    let server = Arc::new(Server::default());
    let request_server = Arc::clone(&server);
    let run_server = thread::spawn(move || serve(listener, &request_server));
    let outcome = Arc::new(Mutex::new(None));
    let result = Arc::clone(&outcome);
    let scenario_server = Arc::clone(&server);
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let checked = async {
                let view = cx.update(|cx| {
                    open_capture_window(
                        cx,
                        CaptureLayout {
                            width: layout.width as f32,
                            height: layout.height as f32,
                            reduced_motion: true,
                        },
                    )
                })?;
                let raw = AnyWindowHandle::from(view);
                prepare_native_window(raw, cx, layout)?;
                view.update(cx, |view, window, cx| {
                    let request = view.state.begin(Intent::Startup);
                    assert!(view.state.finish(
                        &request,
                        Screen::Home(HomeView {
                            session: ActiveSession::Guest,
                            content: HomeContent::Loading,
                            accounts,
                            switch_issue: None,
                        })
                    ));
                    view.show_music_page(Page::Albums, window, cx);
                })?;
                check(view, raw, cx, &scenario_server, &evidence, layout).await
            }
            .await;
            *result.lock().unwrap() = Some(checked);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    server.stopped.store(true, Ordering::Release);
    run_server.join().unwrap();
    store
        .disconnect_account(provider, PROFILE, || Some(()))
        .unwrap();
    outcome.lock().unwrap().take().unwrap().unwrap();
}

fn prepare_native_window(
    raw: AnyWindowHandle,
    cx: &mut AsyncApp,
    layout: Layout,
) -> gpui::Result<()> {
    let (hwnd, rect) = raw.update(cx, |_, window, _| -> gpui::Result<_> {
        let handle = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(io::Error::other("expected HWND").into());
        };
        let hwnd = handle.hwnd.get() as _;
        let mut rect = Default::default();
        if layout.dpi != 96 {
            let mut client = Default::default();
            if unsafe {
                GetClientRect(hwnd, &mut client) == 0 || GetWindowRect(hwnd, &mut rect) == 0
            } {
                return Err(io::Error::last_os_error().into());
            }
            let (width, height) = layout.physical_size();
            rect.right += width as i32 - (client.right - client.left);
            rect.bottom += height as i32 - (client.bottom - client.top);
        }
        Ok((hwnd, rect))
    })??;
    unsafe {
        if layout.dpi != 96 {
            let dpi = layout.dpi as usize;
            SendMessageW(
                hwnd,
                WM_DPICHANGED,
                dpi | dpi << 16,
                &rect as *const _ as isize,
            );
        }
        SendMessageW(hwnd, WM_GETOBJECT, 0, -25);
    }
    Ok(())
}

fn serve(listener: TcpListener, server: &Server) {
    let albums: Vec<_> = (0..ALBUMS)
        .map(|index| {
            json!({"album": {
                "id": 41_000 + index,
                "title": format!("Альбом {index:02}"),
                "artists": [],
            }})
        })
        .collect();
    let reply = json!({"result": albums}).to_string();
    let endpoint = format!("/users/{PROFILE}/likes/albums");
    while !server.stopped.load(Ordering::Acquire) {
        let (mut stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => panic!("{error}"),
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            continue;
        }
        let collection = line.split_whitespace().nth(1) == Some(&endpoint);
        loop {
            line.clear();
            if !matches!(reader.read_line(&mut line), Ok(length) if length > 0)
                || line.trim().is_empty()
            {
                break;
            }
        }
        let (code, body) = if collection {
            let ordinal = 1 + server.received.fetch_add(1, Ordering::AcqRel);
            while server.released.load(Ordering::Acquire) < ordinal
                && !server.stopped.load(Ordering::Acquire)
            {
                thread::sleep(Duration::from_millis(5));
            }
            if ordinal == 4 {
                ("503 Service Unavailable", "{}")
            } else {
                ("200 OK", reply.as_str())
            }
        } else {
            ("404 Not Found", "{}")
        };
        let _ = write!(
            stream,
            "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    }
}

async fn check(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    cx: &mut AsyncApp,
    server: &Server,
    evidence: &Path,
    layout: Layout,
) -> gpui::Result<()> {
    wait(view, raw, cx, |view| {
        server.received.load(Ordering::Acquire) == 1
            && view.music.request.is_some()
            && view.music.collections.results.visible.is_empty()
    })
    .await?;
    let cold = capture(raw, cx, evidence, "cold", layout)?;
    assert_status(&cold, Some("Загружаем коллекции…"));
    server.released.store(1, Ordering::Release);
    wait(view, raw, cx, complete).await?;
    let before = sample(view, cx)?;
    assert_eq!(before.item, 0, "{before:?}");
    assert!(before.row_center.is_some(), "{before:?}");
    assert_status(&capture(raw, cx, evidence, "before-refresh", layout)?, None);

    view.update(cx, |view, _, cx| view.refresh_library(cx))?;
    wait_pending(view, raw, cx, server, 2).await?;
    let pending = sample(view, cx)?;
    let pending_tree = capture(raw, cx, evidence, "cached-pending", layout)?;
    assert_status(&pending_tree, Some("Обновляем коллекции…"));
    assert_eq!(before, pending);
    server.released.store(2, Ordering::Release);
    wait(view, raw, cx, complete).await?;
    let after = sample(view, cx)?;
    assert_eq!(before, after);
    assert_status(&capture(raw, cx, evidence, "same-reply", layout)?, None);

    let viewport = view.update(cx, |view, _, _| view.liked_viewport_for_capture())?;
    raw.update(cx, |_, window, cx| {
        window.dispatch_event(
            ScrollWheelEvent {
                position: viewport.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(-420.))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    })?;
    wait(view, raw, cx, |view| {
        view.music.collections.scroll().offset.item_ix > 1
    })
    .await?;
    let scrolled = sample(view, cx)?;
    assert!(scrolled.anchor.is_some(), "{scrolled:?}");
    view.update(cx, |view, _, cx| view.refresh_library(cx))?;
    wait_pending(view, raw, cx, server, 3).await?;
    assert_eq!(scrolled, sample(view, cx)?);
    server.released.store(3, Ordering::Release);
    wait(view, raw, cx, complete).await?;
    assert_eq!(scrolled, sample(view, cx)?);
    capture(raw, cx, evidence, "scrolled-same-reply", layout)?;

    view.update(cx, |view, _, cx| view.refresh_library(cx))?;
    wait_pending(view, raw, cx, server, 4).await?;
    server.released.store(4, Ordering::Release);
    wait(view, raw, cx, |view| {
        complete(view)
            && view
                .music
                .collections
                .errors
                .contains_key(&ProviderId::YandexMusic)
    })
    .await?;
    // Error/retry sits in the collection header, which is above the current viewport.
    raw.update(cx, |_, window, cx| {
        window.dispatch_event(
            ScrollWheelEvent {
                position: viewport.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(2000.))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    })?;
    wait(view, raw, cx, |view| {
        view.music.collections.scroll().offset.item_ix == 0
    })
    .await?;
    let failed_tree = capture(raw, cx, evidence, "cached-error", layout)?;
    assert_status(&failed_tree, None);
    assert!(node(&failed_tree, "collection-retry").is_some());
    assert_eq!(server.received.load(Ordering::Acquire), 4);

    fs::write(
        evidence.join("geometry.json"),
        serde_json::to_vec_pretty(&json!({
            "viewport_dip": [layout.width, layout.height],
            "dpi": layout.dpi,
            "before": before.json(),
            "pending": pending.json(),
            "same_reply": after.json(),
            "scrolled": scrolled.json(),
            "album_requests": 4,
        }))?,
    )?;
    println!("Collection row and scroll stable: {before:?}; scrolled: {scrolled:?}");
    Ok(())
}

fn complete(view: &MainView) -> bool {
    view.music.request.is_none() && view.music.collections.results.visible.len() == ALBUMS
}

async fn wait_pending(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    cx: &mut AsyncApp,
    server: &Server,
    request: usize,
) -> gpui::Result<()> {
    wait(view, raw, cx, |view| {
        server.received.load(Ordering::Acquire) == request
            && view.music.request.is_some()
            && view.music.collections.results.visible.len() == ALBUMS
    })
    .await
}

async fn wait(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    cx: &mut AsyncApp,
    ready: impl Fn(&MainView) -> bool,
) -> gpui::Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
        if view.update(cx, |view, _, _| ready(view))? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(
                io::Error::other(format!("Collection timed out: {:?}", sample(view, cx)?)).into(),
            );
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
}

#[derive(Debug, PartialEq)]
struct Geometry {
    row_center: Option<[f32; 2]>,
    item: usize,
    offset_in_item: f32,
    anchor: Option<EntryKey>,
}

impl Geometry {
    fn json(&self) -> Value {
        json!({
            "row_center_dip": self.row_center,
            "item": self.item,
            "offset_in_item_dip": self.offset_in_item,
            "anchor": self.anchor.as_ref().map(|value| format!("{value:?}")),
        })
    }
}

fn sample(view: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<Geometry> {
    view.update(cx, |view, _, _| {
        let scroll = view.music.collections.scroll();
        Geometry {
            row_center: view
                .first_library_row_for_capture()
                .map(|center| [center.x.into(), center.y.into()]),
            item: scroll.offset.item_ix,
            offset_in_item: scroll.offset.offset_in_item.into(),
            anchor: scroll.row,
        }
    })
}

fn capture(
    raw: AnyWindowHandle,
    cx: &mut AsyncApp,
    output: &Path,
    name: &str,
    layout: Layout,
) -> gpui::Result<Value> {
    raw.update(cx, |_, window, cx| -> gpui::Result<Value> {
        window.draw(cx).clear(cx);
        let frame = window.render_to_image()?;
        assert_eq!(frame.dimensions(), layout.physical_size());
        assert_eq!(window.scale_factor(), layout.dpi as f32 / 96.);
        frame.save(output.join(format!("{name}.png")))?;
        let tree = window
            .debug_a11y_tree_json()
            .ok_or_else(|| io::Error::other("missing accessibility tree"))?;
        fs::write(output.join(format!("{name}-a11y.json")), &tree)?;
        Ok(serde_json::from_str(&tree)?)
    })?
}

fn node<'a>(tree: &'a Value, id: &str) -> Option<&'a Value> {
    let element = format!("Name(\"{id}\")");
    tree["nodes"]
        .as_object()?
        .values()
        .find(|node| node["element_id"] == element)
}

fn assert_status(tree: &Value, label: Option<&str>) {
    let status = node(tree, "collection-loading");
    assert_eq!(
        status.and_then(|node| node["aria"]["label"].as_str()),
        label
    );
    if let Some(status) = status {
        assert_eq!(status["aria"]["role"], "Status");
    }
}
