use std::{
    collections::BTreeSet,
    env, fs,
    io::{self, BufRead, BufReader, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta, ScrollWheelEvent, WindowHandle, point,
    px,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{Value, json};
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETOBJECT};

use crate::{
    music::ObjectKind,
    providers::{Capabilities, Credentials, Profile, ProviderId, Session, yandex::test_server},
    state::{ActiveSession, HomeContent, HomeView, Intent, Screen},
    storage::Store,
    ui::{CaptureLayout, MainView, application, open_capture_window, workspace::Page},
};

mod artist;
mod listening;

const TEST: &str = "ui::music::prefetch::viewport::native_collection_search::collection_and_search_prefetch_follow_the_native_viewport";
const CHILD: &str = "MEDIADEKA_ENTITY_VIEWPORT_CHILD";
const EVIDENCE: &str = "MEDIADEKA_ENTITY_VIEWPORT_EVIDENCE";
const PROVIDER: ProviderId = ProviderId::YandexMusic;
const PROFILE: &str = "74369";
const LIBRARY: usize = 31_000;
const SEARCH_FIRST: usize = 32_000;
const SEARCH_SECOND: usize = 33_000;

#[test]
#[ignore = "renders a native collection and search with a real catalog adapter over loopback"]
fn collection_and_search_prefetch_follow_the_native_viewport() {
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
    fs::create_dir_all(&evidence).unwrap();
    let store = Store::open_default().unwrap();
    let session = Session {
        provider: PROVIDER,
        profile: Profile {
            id: PROFILE.into(),
            name: "Viewport catalog".into(),
        },
        credentials: Credentials::YandexMusic {
            access_token: "fixture-entity-viewport".into(),
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
    listening::seed(&store);
    let accounts = store.accounts().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    test_server::use_loopback(listener.local_addr().unwrap());
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&stopped);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let received = Arc::clone(&requests);
    let server = thread::spawn(move || serve(listener, &stop, &received));
    let result = Arc::new(Mutex::new(None));
    let outcome = Arc::clone(&result);
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let checked = async {
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
                window.update(cx, |view, window, cx| {
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
                check(window, cx, &evidence, &requests).await
            }
            .await;
            *outcome.lock().unwrap() = Some(checked);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    stopped.store(true, Ordering::Relaxed);
    server.join().unwrap();
    store
        .disconnect_account(PROVIDER, PROFILE, || Some(()))
        .unwrap();
    result.lock().unwrap().take().unwrap().unwrap();
}

fn album(id: usize) -> Value {
    json!({"id": id, "title": format!("Альбом {id}"), "artists":[]})
}

fn serve(listener: TcpListener, stopped: &AtomicBool, requests: &Mutex<Vec<usize>>) {
    while !stopped.load(Ordering::Relaxed) {
        let (mut stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(4));
                continue;
            }
            Err(error) => panic!("{error}"),
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let Some(path) = line.split_whitespace().nth(1).map(str::to_owned) else {
            continue;
        };
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                break;
            }
        }
        let album_id = path
            .strip_prefix("/albums/")
            .and_then(|path| path.strip_suffix("/with-tracks"))
            .and_then(|id| id.parse::<usize>().ok());
        let (code, data) = if let Some(id) = album_id {
            requests.lock().unwrap().push(id);
            ("200 OK", json!({"result": artist::release(id)}))
        } else if let Some(id) = path
            .strip_prefix("/artists/")
            .and_then(|path| path.strip_suffix("/brief-info"))
            .and_then(|id| id.parse().ok())
        {
            requests.lock().unwrap().push(id);
            ("200 OK", artist::brief(id))
        } else if let Some(id) = path.strip_prefix("/tracks/").and_then(|id| id.parse().ok()) {
            requests.lock().unwrap().push(id);
            ("200 OK", json!({"result": [listening::track_data(id)]}))
        } else if path == listening::CONTINUE_PATH {
            requests.lock().unwrap().push(listening::CONTINUE);
            (
                "200 OK",
                json!({"result": {
                    "title": listening::CONTINUE_TITLE,
                    "tracks": [listening::track_data(listening::HISTORY.start)]
                }}),
            )
        } else if path == format!("/users/{PROFILE}/likes/albums") {
            let items = (LIBRARY..LIBRARY + 96)
                .map(|id| json!({"album": album(id)}))
                .collect::<Vec<_>>();
            ("200 OK", json!({"result":items}))
        } else if path.starts_with("/search?") && path.contains("type=album") {
            let base = if path.contains("second") {
                SEARCH_SECOND
            } else {
                SEARCH_FIRST
            };
            let items = (base..base + 20).map(album).collect::<Vec<_>>();
            ("200 OK", json!({"result":{"albums":{"results":items}}}))
        } else {
            ("404 Not Found", json!({}))
        };
        let body = data.to_string();
        let _ = write!(
            stream,
            "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Sample {
    visible: Vec<usize>,
    nearby: Vec<usize>,
}

impl Sample {
    fn read(view: &MainView) -> Self {
        let scheduler = &view.music.prefetch;
        let id = |key: &super::Key| {
            assert_eq!(key.profile.as_deref(), Some(PROFILE));
            assert_eq!(key.object.provider, PROVIDER);
            if key.object.kind == ObjectKind::Playlist {
                assert_eq!(key.object.id, listening::CONTINUE_ID);
                return listening::CONTINUE;
            }
            let id = key.object.id.parse().unwrap();
            let kind = if artist::ARTISTS.contains(&id) {
                ObjectKind::Artist
            } else if listening::HISTORY.contains(&id) {
                ObjectKind::Track
            } else {
                ObjectKind::Album
            };
            assert_eq!(key.object.kind, kind);
            id
        };
        Self {
            visible: scheduler.viewport.visible().iter().map(id).collect(),
            nearby: scheduler.page.iter().map(|job| id(&job.key)).collect(),
        }
    }
}

async fn check(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    evidence: &Path,
    requests: &Mutex<Vec<usize>>,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    enable_accessibility(raw, cx)?;
    let library = wait(window, cx, requests, |sample| {
        sample
            .visible
            .iter()
            .all(|id| (LIBRARY..LIBRARY + 10).contains(id))
    })
    .await?;
    capture(raw, cx, evidence, "library-initial")?;
    scroll(raw, -690., cx)?;
    let forward = wait(window, cx, requests, |sample| {
        !sample.nearby.is_empty()
            && sample
                .visible
                .iter()
                .all(|id| !library.visible.contains(id))
    })
    .await?;
    capture(raw, cx, evidence, "library-forward")?;
    scroll(raw, 220., cx)?;
    let backward = wait(window, cx, requests, |sample| {
        !sample.nearby.is_empty()
            && sample.visible.first() < forward.visible.first()
            && sample.nearby.iter().all(|id| !forward.nearby.contains(id))
    })
    .await?;
    capture(raw, cx, evidence, "library-backward")?;
    assert!(forward.nearby.iter().min() > forward.visible.iter().max());
    assert!(backward.nearby.iter().max() < backward.visible.iter().min());

    let (track, thumb) = window
        .update(cx, |view, window, _| {
            view.library_scrollbar_for_test(window)
        })?
        .expect("library scrollbar is visible");
    let track_click = point(track.center().x, track.top() + track.size.height * 0.72);
    assert!(track_click.y > thumb + px(32.));
    scrollbar_down(raw, track_click, cx)?;
    scrollbar_up(raw, track_click, cx)?;
    assert!(window.update(cx, |view, _, _| Sample::read(view).nearby.is_empty())?);
    let clicked = wait(window, cx, requests, |sample| {
        sample.visible.iter().min() > backward.visible.iter().max()
            && sample.nearby.iter().min() > sample.visible.iter().max()
    })
    .await?;
    capture(raw, cx, evidence, "library-scrollbar-click")?;

    let (_, thumb) = window
        .update(cx, |view, window, _| {
            view.library_scrollbar_for_test(window)
        })?
        .expect("library scrollbar is visible after click");
    let grab = point(track.center().x, thumb);
    let drag_to = point(grab.x, track.top() + track.size.height * 0.43);
    scrollbar_down(raw, grab, cx)?;
    assert_eq!(window.update(cx, |view, _, _| Sample::read(view))?, clicked);
    scrollbar_move(raw, drag_to, cx)?;
    assert!(window.update(cx, |view, _, _| Sample::read(view).nearby.is_empty())?);
    let dragged = wait(window, cx, requests, |sample| {
        !sample.nearby.is_empty()
            && sample.visible.iter().max() < clicked.visible.iter().min()
            && sample.nearby.iter().max() < sample.visible.iter().min()
    })
    .await?;
    capture(raw, cx, evidence, "library-scrollbar-drag")?;
    let requests_before_release = requests.lock().unwrap().len();
    scrollbar_move(raw, drag_to, cx)?;
    scrollbar_up(raw, drag_to, cx)?;
    cx.background_executor()
        .timer(Duration::from_millis(300))
        .await;
    assert_eq!(window.update(cx, |view, _, _| Sample::read(view))?, dragged);
    assert_eq!(requests.lock().unwrap().len(), requests_before_release);

    window.update(cx, |view, _, cx| {
        view.music
            .filter
            .update(cx, |field, cx| field.set_text("31042", cx));
    })?;
    let filtered = wait(window, cx, requests, |sample| {
        sample.visible == [31_042] && sample.nearby.is_empty()
    })
    .await?;
    capture(raw, cx, evidence, "library-filtered")?;

    open_search(window, "first", cx)?;
    let initial = wait(window, cx, requests, |sample| {
        sample.nearby.is_empty()
            && sample
                .visible
                .iter()
                .all(|id| (SEARCH_FIRST..SEARCH_FIRST + 5).contains(id))
    })
    .await?;
    capture(raw, cx, evidence, "search-initial")?;
    scroll(raw, -510., cx)?;
    let scrolled = wait(window, cx, requests, |sample| {
        sample
            .visible
            .iter()
            .all(|id| (SEARCH_FIRST + 5..SEARCH_FIRST + 20).contains(id))
    })
    .await?;
    capture(raw, cx, evidence, "search-scrolled")?;
    open_search(window, "second", cx)?;
    let replaced = wait(window, cx, requests, |sample| {
        sample
            .visible
            .iter()
            .all(|id| (SEARCH_SECOND..SEARCH_SECOND + 5).contains(id))
    })
    .await?;
    capture(raw, cx, evidence, "search-replaced")?;
    let expected: BTreeSet<_> = [
        &library, &forward, &backward, &clicked, &dragged, &filtered, &initial, &scrolled,
        &replaced,
    ]
    .into_iter()
    .flat_map(|sample| sample.visible.iter().chain(&sample.nearby).copied())
    .collect();
    let requested = requests.lock().unwrap().clone();
    assert_eq!(requested.iter().copied().collect::<BTreeSet<_>>(), expected);
    assert_eq!(
        requested.len(),
        expected.len(),
        "a catalog request was repeated"
    );
    let report = format!(
        "Library initial {library:?}\nLibrary forward {forward:?}\nLibrary backward {backward:?}\nLibrary scrollbar track click {clicked:?}\nLibrary scrollbar thumb drag {dragged:?}\nLibrary filtered {filtered:?}\nSearch initial {initial:?}\nSearch scrolled {scrolled:?}\nSearch replaced {replaced:?}\nHTTP album ids {requested:?}\n"
    );
    print!("{report}");
    fs::write(evidence.join("viewport.txt"), report)?;
    artist::check(window, cx, evidence, requests).await?;
    listening::check(window, cx, evidence, requests).await
}

fn open_search(window: WindowHandle<MainView>, query: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    window.update(cx, |view, window, cx| {
        view.submit_catalog(ObjectKind::Album, query.into(), window, cx);
        let sample = Sample::read(view);
        assert!(sample.visible.is_empty() && sample.nearby.is_empty());
    })
}

async fn wait(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    requests: &Mutex<Vec<usize>>,
    expected: impl Fn(&Sample) -> bool,
) -> gpui::Result<Sample> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let raw = AnyWindowHandle::from(window);
    loop {
        raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
        let sample = window.update(cx, |view, _, _| Sample::read(view))?;
        let requested = requests.lock().unwrap().clone();
        if !sample.visible.is_empty()
            && expected(&sample)
            && sample
                .visible
                .iter()
                .chain(&sample.nearby)
                .all(|id| requested.contains(id))
        {
            return Ok(sample);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "no expected viewport, {sample:?}, HTTP {requested:?}"
            ))
            .into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
    }
}

fn scroll(raw: AnyWindowHandle, vertical: f32, cx: &mut AsyncApp) -> gpui::Result<()> {
    raw.update(cx, |_, window, cx| {
        let position = point(px(345.), px(235.));
        window.dispatch_event(
            MouseMoveEvent {
                position,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(point(px(0.), px(vertical))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    })
}

fn scrollbar_down(
    raw: AnyWindowHandle,
    position: Point<Pixels>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    dispatch(
        raw,
        MouseMoveEvent {
            position,
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    )?;
    dispatch(
        raw,
        MouseDownEvent {
            position,
            button: MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    )
}

fn scrollbar_move(
    raw: AnyWindowHandle,
    position: Point<Pixels>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    dispatch(
        raw,
        MouseMoveEvent {
            position,
            pressed_button: Some(MouseButton::Left),
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    )
}

fn scrollbar_up(
    raw: AnyWindowHandle,
    position: Point<Pixels>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    dispatch(
        raw,
        MouseUpEvent {
            position,
            button: MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    )
}

fn dispatch(raw: AnyWindowHandle, event: PlatformInput, cx: &mut AsyncApp) -> gpui::Result<()> {
    raw.update(cx, |_, window, cx| {
        window.dispatch_event(event, cx);
    })
}

fn enable_accessibility(raw: AnyWindowHandle, cx: &mut AsyncApp) -> gpui::Result<()> {
    raw.update(cx, |_, window, _| -> gpui::Result<()> {
        let handle = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(io::Error::other("expected Win32").into());
        };
        unsafe { SendMessageW(handle.hwnd.get() as _, WM_GETOBJECT, 0, -25) };
        Ok(())
    })?
}

fn capture(raw: AnyWindowHandle, cx: &mut AsyncApp, output: &Path, name: &str) -> gpui::Result<()> {
    raw.update(cx, |_, window, cx| -> gpui::Result<()> {
        window.draw(cx).clear(cx);
        window
            .render_to_image()?
            .save(output.join(format!("{name}.png")))?;
        fs::write(
            output.join(format!("{name}-a11y.json")),
            window
                .debug_a11y_tree_json()
                .ok_or_else(|| io::Error::other("missing accessibility tree"))?,
        )?;
        Ok(())
    })?
}
