use std::{
    env, fs,
    io::{self, BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU16, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, InputEvent, KeyUpEvent, Keystroke, WindowHandle};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{Value, json};
use url::Url;
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETOBJECT};

use crate::{
    providers::{
        Capabilities, Credentials, Profile, ProviderId, Session, use_soundcloud_loopback_for_test,
    },
    storage::Store,
    ui::{CaptureLayout, MainView, application, open_capture_window},
};

const TEST: &str = "ui::music::search::native_soundcloud::public_soundcloud_set_recovers_the_same_url_and_opens_the_confirmed_route";
const CHILD: &str = "MEDIADEKA_SOUNDCLOUD_SET_CHILD";
const EVIDENCE: &str = "MEDIADEKA_SOUNDCLOUD_SET_EVIDENCE";
const PROFILE: &str = "8274516";
const ALBUM: &str = "https://soundcloud.com/studio-fixture/sets/album";
const ALBUM_INPUT: &str = "https://m.soundcloud.com/studio-fixture/sets/album?utm_source=copy_link";
const PLAYLIST: &str = "https://soundcloud.com/studio-fixture/sets/playlist";
const CONNECT: &str = "music-search-source-connect";
const RETRY: &str = "music-search-source-retry";
const OPEN: &str = "music-search-open-url";

#[test]
#[ignore = "uses a Windows D3D11 window, an isolated profile and the SoundCloud adapter over loopback"]
fn public_soundcloud_set_recovers_the_same_url_and_opens_the_confirmed_route() {
    if env::var_os(CHILD).is_none() {
        let profile = tempfile::tempdir().unwrap();
        assert!(
            Command::new(env::current_exe().unwrap())
                .args(["--exact", TEST, "--ignored", "--nocapture"])
                .env(CHILD, "1")
                .env("MEDIADEKA_PROFILE_DIR", profile.path())
                .env("MEDIADEKA_GUI", "3")
                .env("HTTP_PROXY", "http://127.0.0.1:1")
                .env("HTTPS_PROXY", "http://127.0.0.1:1")
                .env("NO_PROXY", "127.0.0.1")
                .status()
                .unwrap()
                .success()
        );
        return;
    }
    let evidence = PathBuf::from(env::var_os(EVIDENCE).unwrap());
    fs::create_dir_all(&evidence).unwrap();
    let store = Store::open_default().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    use_soundcloud_loopback_for_test(listener.local_addr().unwrap());
    let fixture = Arc::new(Server::default());
    let loopback = fixture.clone();
    let server = thread::spawn(move || loopback.serve(listener));
    let outcome = Arc::new(Mutex::new(None));
    let result = outcome.clone();
    let checks = fixture.clone();
    let profile = store.clone();
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let result = async {
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
                check(main, cx, &profile, &checks, &evidence).await
            }
            .await;
            *outcome.lock().unwrap() = Some(result);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    fixture.stopped.store(true, Ordering::Relaxed);
    server.join().unwrap();
    store
        .disconnect_account(ProviderId::SoundCloud, PROFILE, || Some(()))
        .unwrap();
    result.lock().unwrap().take().unwrap().unwrap();
}

#[derive(Default)]
struct Server {
    stopped: AtomicBool,
    status: AtomicU16,
    resolved: Mutex<Vec<(u16, String)>>,
}

impl Server {
    fn serve(&self, listener: TcpListener) {
        while !self.stopped.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => self.respond(stream).unwrap(),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("{error}"),
            }
        }
    }

    fn respond(&self, mut stream: TcpStream) -> io::Result<()> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let path = line
            .split_whitespace()
            .nth(1)
            .ok_or_else(|| io::Error::other("request line was empty"))?;
        let url = Url::parse(&format!("http://localhost{path}")).unwrap();
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
                break;
            }
        }
        let (status, body) = match url.path() {
            "/resolve" => {
                let link = url
                    .query_pairs()
                    .find(|(key, _)| key == "url")
                    .unwrap()
                    .1
                    .into_owned();
                assert!(matches!(link.as_str(), ALBUM | PLAYLIST));
                let status = self.status.load(Ordering::Relaxed);
                self.resolved.lock().unwrap().push((status, link.clone()));
                match status {
                    200 => ("200 OK", collection(link == ALBUM)),
                    401 => ("401 Unauthorized", json!({})),
                    503 => ("503 Service Unavailable", json!({})),
                    _ => panic!("classification attempted before a test connection"),
                }
            }
            "/me" => (
                "200 OK",
                json!({"id":PROFILE.parse::<u32>().unwrap(),"username":"SoundCloud QA"}),
            ),
            "/playlists/871" => ("200 OK", collection(true)),
            "/playlists/872" => ("200 OK", collection(false)),
            _ => ("200 OK", json!({"collection":[]})),
        };
        let body = body.to_string();
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }
}

fn collection(album: bool) -> Value {
    let (id, title, url) = if album {
        (871, "Проверенный альбом", ALBUM)
    } else {
        (872, "Проверенный плейлист", PLAYLIST)
    };
    json!({"kind":"playlist","id":id,"title":title,"permalink_url":url,"is_album":album,"tracks":[]})
}

async fn check(
    main: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    store: &Store,
    server: &Server,
    evidence: &Path,
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
    main.update(cx, |view, window, cx| {
        view.start_home_for_capture(false, window, cx)
    })?;
    wait(cx, |cx| {
        Ok(main
            .update(cx, |view, _, _| view.home_status_for_capture())?
            .is_some())
    })
    .await?;
    submit(main, ALBUM_INPUT, cx)?;
    wait_node(raw, CONNECT, cx).await?;
    let guest = capture(raw, cx, evidence, "guest-recovery")?;
    assert_eq!(
        node(&guest, CONNECT).unwrap()["aria"]["label"],
        "Подключить SoundCloud"
    );
    assert_query(&guest, ALBUM_INPUT);
    assert!(node(&guest, OPEN).is_none());
    assert!(server.resolved.lock().unwrap().is_empty());
    press(raw, CONNECT, cx)?;
    wait_node(raw, "settings-login-modal", cx).await?;
    let settings = capture(raw, cx, evidence, "soundcloud-connection")?;
    assert_eq!(
        node(&settings, "login-panel").unwrap()["aria"]["label"],
        "SoundCloud"
    );
    key(raw, "escape", cx)?;
    press(raw, "workspace-back", cx)?;
    wait_node(raw, CONNECT, cx).await?;
    assert_query(
        &capture(raw, cx, evidence, "return-to-same-link")?,
        ALBUM_INPUT,
    );
    press(raw, CONNECT, cx)?;
    wait_node(raw, "settings-login-modal", cx).await?;
    key(raw, "escape", cx)?;

    server.status.store(503, Ordering::Relaxed);
    let fixture = Session {
        provider: ProviderId::SoundCloud,
        profile: Profile {
            id: PROFILE.into(),
            name: "SoundCloud QA".into(),
        },
        capabilities: Capabilities {
            home: true,
            profile_selection: false,
            device_code: false,
        },
        credentials: Credentials::SoundCloud {
            token: "synthetic-loopback-session".into(),
            client_id: "0123456789abcdef0123456789abcdef".into(),
        },
    };
    store.use_account(&fixture, || Some(()), || {})?;
    main.update(cx, |view, window, cx| {
        view.start_home_for_capture(true, window, cx)
    })?;
    wait(cx, |cx| {
        main.update(cx, |view, _, _| {
            view.home_status_for_capture().is_some()
                && view.connected_provider_profile(ProviderId::SoundCloud) == Some(PROFILE)
        })
    })
    .await?;
    press(raw, "workspace-back", cx)?;
    wait_node(raw, RETRY, cx).await?;
    let temporary = capture(raw, cx, evidence, "transport-retry")?;
    assert_query(&temporary, ALBUM_INPUT);
    assert!(node(&temporary, OPEN).is_none());
    server.status.store(200, Ordering::Relaxed);
    press(raw, RETRY, cx)?;
    wait_node(raw, OPEN, cx).await?;
    assert_query(
        &capture(raw, cx, evidence, "resolved-album-link")?,
        ALBUM_INPUT,
    );
    press(raw, OPEN, cx)?;
    wait(cx, |cx| {
        Ok(
            main.update(cx, |view, _, _| view.music_semantics_for_capture())?["details"]["kind"]
                == "album",
        )
    })
    .await?;
    capture(raw, cx, evidence, "confirmed-album")?;

    server.status.store(401, Ordering::Relaxed);
    submit(main, PLAYLIST, cx)?;
    wait_node(raw, CONNECT, cx).await?;
    let expired = capture(raw, cx, evidence, "expired-recovery")?;
    assert_query(&expired, PLAYLIST);
    assert!(node(&expired, OPEN).is_none());
    server.status.store(200, Ordering::Relaxed);
    submit(main, PLAYLIST, cx)?;
    wait_node(raw, OPEN, cx).await?;
    press(raw, OPEN, cx)?;
    wait(cx, |cx| {
        Ok(main.update(cx, |view, _, _| view.playlist_status_for_capture())? == Some(Ok(0)))
    })
    .await?;
    capture(raw, cx, evidence, "confirmed-playlist")?;
    let requests = server.resolved.lock().unwrap().clone();
    assert_eq!(
        requests,
        [
            (503, ALBUM.into()),
            (200, ALBUM.into()),
            (401, PLAYLIST.into()),
            (200, PLAYLIST.into())
        ]
    );
    fs::write(
        evidence.join("soundcloud-set.json"),
        serde_json::to_vec_pretty(&requests)?,
    )?;
    Ok(())
}

fn assert_query(tree: &Value, expected: &str) {
    assert_eq!(
        node(tree, "home-search").unwrap()["aria"]["value"],
        expected
    );
}

fn submit(main: WindowHandle<MainView>, input: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    main.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(input, window, cx)
    })
}

fn node<'a>(tree: &'a Value, id: &str) -> Option<&'a Value> {
    let id = format!("Name({id:?})");
    tree["nodes"]
        .as_object()?
        .values()
        .find(|item| item["element_id"] == id)
}

async fn wait_node(raw: AnyWindowHandle, id: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    wait(cx, |cx| Ok(node(&frame(raw, cx, None)?, id).is_some()))
        .await
        .map_err(|error| io::Error::other(format!("{id}: {error}")).into())
}

async fn wait(
    cx: &mut AsyncApp,
    condition: impl Fn(&mut AsyncApp) -> gpui::Result<bool>,
) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition(cx)? {
        if Instant::now() >= deadline {
            return Err(io::Error::other("expected SoundCloud state was not reached").into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
    }
    Ok(())
}

fn press(raw: AnyWindowHandle, id: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    let id = format!("Name({id:?})");
    for _ in 0..60 {
        let tree = frame(raw, cx, None)?;
        let focus = tree["gpui_focus"].as_str().unwrap_or_default();
        if tree["nodes"][focus]["element_id"] == id {
            return key(raw, "enter", cx);
        }
        key(raw, "tab", cx)?;
    }
    Err(io::Error::other(format!("keyboard focus did not reach {id}")).into())
}

fn key(raw: AnyWindowHandle, name: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    raw.update(cx, |_, window, cx| {
        let key = Keystroke::parse(name).unwrap();
        window.dispatch_keystroke(key.clone(), cx);
        window.dispatch_event(KeyUpEvent { keystroke: key }.to_platform_input(), cx);
    })
}

fn capture(
    raw: AnyWindowHandle,
    cx: &mut AsyncApp,
    evidence: &Path,
    name: &str,
) -> gpui::Result<Value> {
    frame(raw, cx, Some(&evidence.join(format!("{name}.png"))))
}

fn frame(raw: AnyWindowHandle, cx: &mut AsyncApp, output: Option<&Path>) -> gpui::Result<Value> {
    raw.update(cx, |_, window, cx| -> gpui::Result<Value> {
        window.draw(cx).clear(cx);
        let tree = window
            .debug_a11y_tree_json()
            .ok_or_else(|| io::Error::other("missing accessibility tree"))?;
        if let Some(file) = output {
            window.render_to_image()?.save(file)?;
            fs::write(file.with_extension("json"), &tree)?;
        }
        Ok(serde_json::from_str(&tree)?)
    })?
}
