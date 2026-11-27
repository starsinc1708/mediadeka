use std::{
    env, fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, InputEvent, KeyUpEvent, Keystroke, WindowHandle};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{Value, json};
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETOBJECT};

use crate::{
    music::{ObjectKind, ObjectRef},
    playlist::{Media, PlaylistKey, PlaylistTrack},
    providers::{Capabilities, Credentials, Profile, ProviderId, Session, yandex::test_server},
    state::{ActiveSession, HomeContent, HomeView, Intent, Screen},
    storage::{Account, Store},
    ui::{CaptureLayout, MainView, application, open_capture_window, workspace::MusicTarget},
};

use super::LikeIdentity;

const TEST: &str = "ui::likes::native::optimistic_like_follows_delayed_native_yandex_writes";
const CHILD: &str = "MEDIADEKA_OPTIMISTIC_LIKE_CHILD";
const EVIDENCE: &str = "MEDIADEKA_OPTIMISTIC_LIKE_EVIDENCE";
const PROVIDER: ProviderId = ProviderId::YandexMusic;
const PROFILE: &str = "43692179";
const TRACK: &str = "634921";
const TITLE: &str = "Трек для проверки лайков";
const HEART: &str = "music-object-track-like";
const LOCAL: &str = "like-destination-mediadeka";
const REMOTE: &str = "like-destination-yandex-music";

#[test]
#[ignore = "uses a native GPUI window and a delayed loopback for real Yandex writes"]
fn optimistic_like_follows_delayed_native_yandex_writes() {
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
            name: "Native likes fixture".into(),
        },
        credentials: Credentials::YandexMusic {
            access_token: "fixture-native-likes".into(),
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
    let account = store.account(PROVIDER, PROFILE).unwrap().unwrap();
    let accounts = store.accounts().unwrap();
    store.set_like_defaults(&[None, Some(PROVIDER)]).unwrap();
    let key = PlaylistKey::Source {
        provider: PROVIDER,
        profile: Some(PROFILE.into()),
        id: TRACK.into(),
    };
    let track = PlaylistTrack {
        id: TRACK.into(),
        title: TITLE.into(),
        artist: String::new(),
        artists: Vec::new(),
        artist_credits: Vec::new(),
        album: None,
        album_ref: None,
        artwork: None,
        added: None,
        duration: Some(180),
        media: Some(Media::Provider {
            provider: PROVIDER,
            id: TRACK.into(),
        }),
    };
    let identity = LikeIdentity::from_track(&key, &track).unwrap();
    store
        .set_local_like(&identity, &track, true, &AtomicBool::new(false))
        .unwrap();
    store.record_remote_like(&account, &track, false).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    test_server::use_loopback(listener.local_addr().unwrap());
    let (writes, requests) = mpsc::channel();
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let server = thread::spawn(move || serve(listener, &stop, &writes));
    let result = Arc::new(Mutex::new(None));
    let outcome = result.clone();
    let native = Fixture {
        store: store.clone(),
        account,
        identity,
        requests,
        evidence,
    };
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let checked = async {
                let main = cx.update(|cx| {
                    open_capture_window(
                        cx,
                        CaptureLayout {
                            width: 800.,
                            height: 400.,
                            reduced_motion: true,
                        },
                    )
                })?;
                main.update(cx, |view, window, cx| {
                    let startup = view.state.begin(Intent::Startup);
                    assert!(view.state.finish(
                        &startup,
                        Screen::Home(HomeView {
                            session: ActiveSession::Guest,
                            content: HomeContent::Loading,
                            accounts,
                            switch_issue: None,
                        })
                    ));
                    view.sync_like_accounts(cx);
                    view.load_like_defaults(cx);
                    view.open_music_target(
                        MusicTarget {
                            object: ObjectRef::native(PROVIDER, ObjectKind::Track, TRACK.into())
                                .unwrap(),
                            profile: Some(PROFILE.into()),
                            title: String::new(),
                            artwork: None,
                        },
                        window,
                        cx,
                    );
                })?;
                check(main, cx, native).await
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

struct Fixture {
    store: Store,
    account: Account,
    identity: LikeIdentity,
    requests: Receiver<WriteRequest>,
    evidence: PathBuf,
}

struct WriteRequest {
    liked: bool,
    response: Sender<bool>,
}

fn serve(listener: TcpListener, stopped: &AtomicBool, writes: &Sender<WriteRequest>) {
    while !stopped.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => respond(stream, writes).unwrap(),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("{error}"),
        }
    }
}

fn respond(mut stream: TcpStream, writes: &Sender<WriteRequest>) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let request = line.trim().to_owned();
    let mut length = 0;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':')
            && key.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().unwrap();
        }
    }
    let mut received = vec![0; length];
    reader.read_exact(&mut received)?;
    let path = request.split_whitespace().nth(1).unwrap_or_default();
    let mutation_path = format!("/users/{PROFILE}/likes/tracks/");
    let (status, value) = if let Some(action) = path.strip_prefix(&mutation_path) {
        assert!(request.starts_with("POST "));
        assert_eq!(received, format!("track-ids={TRACK}").as_bytes());
        let liked = match action {
            "add-multiple" => true,
            "remove" => false,
            _ => panic!("unrecognized like operation"),
        };
        let (response, result) = mpsc::channel();
        let ok = writes.send(WriteRequest { liked, response }).is_ok()
            && result.recv_timeout(Duration::from_secs(8)).unwrap_or(false);
        if ok {
            ("200 OK", json!({"result":{"revision":3}}))
        } else {
            ("503 Service Unavailable", json!({}))
        }
    } else if path == "/account/status" {
        (
            "200 OK",
            json!({"result":{"account":{"uid":PROFILE,"displayName":"Native likes fixture"}}}),
        )
    } else if path == format!("/tracks/{TRACK}") {
        (
            "200 OK",
            json!({"result":[{"id":TRACK,"title":TITLE,"durationMs":180000,"artists":[],"albums":[]}]}),
        )
    } else if path == format!("/users/{PROFILE}/likes/tracks") {
        ("200 OK", json!({"result":{"library":{"tracks":[]}}}))
    } else {
        ("404 Not Found", json!({}))
    };
    let body = value.to_string();
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

async fn check(
    main: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    fixture: Fixture,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(main);
    raw.update(cx, |_, window, _| -> gpui::Result<()> {
        let handle = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(io::Error::other("expected Win32").into());
        };
        unsafe { SendMessageW(handle.hwnd.get() as _, WM_GETOBJECT, 0, -25) };
        Ok(())
    })??;
    wait_tree(raw, cx, |tree| !node(tree, HEART).is_null()).await?;
    assert!(main.update(cx, |view, _, _| view.likes.defaults
        == [None, Some(PROVIDER)])?);
    focus(raw, HEART, cx)?;
    press(raw, "shift-f10", cx)?;
    wait_tree(raw, cx, |tree| node(tree, REMOTE)["toggled"] == "False").await?;
    press(raw, "escape", cx)?;
    focus(raw, HEART, cx)?;
    press(raw, "enter", cx)?;

    for (liked, accepted, caption) in [
        (true, false, "add-rejected"),
        (true, true, "add-confirmed"),
        (false, false, "remove-rejected"),
        (false, true, "remove-confirmed"),
    ] {
        if caption != "add-rejected" {
            focus(raw, REMOTE, cx)?;
            press(raw, "enter", cx)?;
        }
        let request = wait_write(&fixture.requests, cx).await?;
        assert_eq!(request.liked, liked);
        assert_eq!(
            fixture
                .store
                .remote_like_override(&fixture.account, TRACK)?,
            Some(!liked)
        );
        assert!(fixture.store.local_like(&fixture.identity)?);
        assert!(main.update(cx, |view, _, _| {
            view.likes
                .pending_mark(&fixture.identity, Some(PROVIDER), Some(PROFILE))
                == Some(liked)
                && view
                    .likes
                    .pending_mark(&fixture.identity, Some(PROVIDER), Some("another-profile"))
                    .is_none()
        })?);
        if caption == "add-rejected" {
            wait_tree(raw, cx, |tree| {
                node(tree, HEART)["toggled"] == "True"
                    && node(tree, HEART)["label"]
                        .as_str()
                        .is_some_and(|text| text.contains("ожидает"))
            })
            .await?;
            capture(raw, cx, &fixture.evidence, "quick-add-pending")?;
            press(raw, "enter", cx)?;
            press(raw, "shift-f10", cx)?;
        }
        let state = if liked { "True" } else { "False" };
        let verb = if liked {
            "Сохраняем…"
        } else {
            "Убираем…"
        };
        wait_tree(raw, cx, |tree| {
            node(tree, LOCAL)["toggled"] == "True"
                && node(tree, REMOTE)["toggled"] == state
                && node(tree, REMOTE)["label"]
                    .as_str()
                    .is_some_and(|text| text.contains(verb))
                && node(tree, HEART)["toggled"] == state
        })
        .await?;
        capture(raw, cx, &fixture.evidence, &format!("{caption}-pending"))?;
        press(raw, "enter", cx)?;
        assert!(
            fixture.requests.try_recv().is_err(),
            "repeated action while pending"
        );
        request.response.send(accepted).unwrap();
        let committed = if accepted { liked } else { !liked };
        let state = if committed { "True" } else { "False" };
        wait_tree(raw, cx, |tree| {
            node(tree, LOCAL)["toggled"] == "True"
                && node(tree, REMOTE)["toggled"] == state
                && node(tree, HEART)["toggled"] == state
                && !node(tree, REMOTE)["label"]
                    .as_str()
                    .is_some_and(|text| text.contains(verb))
                && node(tree, "like-error-yandex-music").is_null() == accepted
        })
        .await?;
        capture(raw, cx, &fixture.evidence, caption)?;
        assert_eq!(
            fixture
                .store
                .remote_like_override(&fixture.account, TRACK)?,
            Some(committed)
        );
        assert!(fixture.store.local_like(&fixture.identity)?);
    }
    assert!(fixture.requests.try_recv().is_err());
    fs::write(
        fixture.evidence.join("optimistic-likes.txt"),
        "Yandex POST: add → 503, retry add → 200, remove → 503, retry remove → 200.\nEach blocked response exposed the requested state before SQLite commit; failures restored the previous mark. The Mediadeka mark stayed enabled throughout. No repeated POST.\n",
    )?;
    Ok(())
}

async fn wait_write(
    requests: &Receiver<WriteRequest>,
    cx: &mut AsyncApp,
) -> gpui::Result<WriteRequest> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(request) = requests.try_recv() {
            return Ok(request);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Yandex like request did not arrive").into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
}

async fn wait_tree(
    raw: AnyWindowHandle,
    cx: &mut AsyncApp,
    ready: impl Fn(&Value) -> bool,
) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let tree = frame(raw, cx, None)?;
        if ready(&tree) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "Like states: heart {}, remote {}, local {}",
                node(&tree, HEART),
                node(&tree, REMOTE),
                node(&tree, LOCAL)
            ))
            .into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
}

fn focus(raw: AnyWindowHandle, id: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    let expected = format!("Name(\"{id}\")");
    for _ in 0..50 {
        let tree = frame(raw, cx, None)?;
        let current = tree["gpui_focus"].as_str().unwrap_or_default();
        if tree["nodes"][current]["element_id"] == expected {
            return Ok(());
        }
        press(raw, "tab", cx)?;
    }
    Err(io::Error::other(format!("no keyboard focus for {id}")).into())
}

fn press(raw: AnyWindowHandle, key: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    raw.update(cx, |_, window, cx| {
        let key = Keystroke::parse(key).unwrap();
        window.dispatch_keystroke(key.clone(), cx);
        window.dispatch_event(KeyUpEvent { keystroke: key }.to_platform_input(), cx);
    })
}

fn node<'a>(tree: &'a Value, id: &str) -> &'a Value {
    let expected = format!("Name(\"{id}\")");
    tree["nodes"]
        .as_object()
        .unwrap()
        .values()
        .find(|node| node["element_id"] == expected)
        .map_or(&Value::Null, |node| &node["aria"])
}

fn capture(
    raw: AnyWindowHandle,
    cx: &mut AsyncApp,
    evidence: &Path,
    name: &str,
) -> gpui::Result<()> {
    fs::write(
        evidence.join(format!("{name}-a11y.json")),
        serde_json::to_string_pretty(&frame(
            raw,
            cx,
            Some(&evidence.join(format!("{name}.png"))),
        )?)?,
    )?;
    Ok(())
}

fn frame(raw: AnyWindowHandle, cx: &mut AsyncApp, output: Option<&Path>) -> gpui::Result<Value> {
    raw.update(cx, |_, window, cx| -> gpui::Result<Value> {
        window.draw(cx).clear(cx);
        if let Some(path) = output {
            window.render_to_image()?.save(path)?;
        }
        let tree = window
            .debug_a11y_tree_json()
            .ok_or_else(|| io::Error::other("missing accessibility tree"))?;
        Ok(serde_json::from_str(&tree)?)
    })?
}
