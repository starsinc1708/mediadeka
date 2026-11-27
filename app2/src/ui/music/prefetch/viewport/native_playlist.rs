use std::{
    env, fs,
    io::{self, BufRead, BufReader, Cursor, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Instant,
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PlatformInput, WindowHandle, point,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{Value, json};
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETOBJECT};

use crate::{
    music::{ObjectKind, ObjectRef, library::PlaylistScope},
    playlist::{Media, Playlist, PlaylistKey, PlaylistTrack},
    providers::{Capabilities, Credentials, Profile, ProviderId, Session, yandex::test_server},
    storage::Store,
    ui::{CaptureLayout, application, components::artwork, open_capture_window, workspace::Page},
};

use super::*;

const CHILD: &str = "MEDIADEKA_PLAYLIST_VIEWPORT_CHILD";
const EVIDENCE: &str = "MEDIADEKA_PLAYLIST_VIEWPORT_EVIDENCE";
const TEST: &str = "ui::music::prefetch::viewport::native_playlist::native_playlist_prefetch_uses_the_rendered_rows";
const PROVIDER: ProviderId = ProviderId::YandexMusic;
const PROFILE: &str = "74368";
const PLAYLIST: &str = "74368:415";
const ALBUM_BASE: usize = 20_000;
const TRACKS: usize = 96;
const PLAYLIST_COVER: &str = "https://i.scdn.co/image/mediadeka-playlist";
const AHEAD_COVER: &str = "https://i.scdn.co/image/mediadeka-ahead";
const BEHIND_COVER: &str = "https://i.scdn.co/image/mediadeka-behind";
const FAR_COVER: &str = "https://i.scdn.co/image/mediadeka-far";

#[test]
#[ignore = "requires native Windows D3D11; uses a local HTTP fixture for the Yandex adapter"]
fn native_playlist_prefetch_uses_the_rendered_rows() {
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

    let evidence = PathBuf::from(env::var_os(EVIDENCE).unwrap());
    fs::create_dir_all(&evidence).unwrap();
    let profile = PathBuf::from(env::var_os("MEDIADEKA_PROFILE_DIR").unwrap());
    let fixture = evidence.join("artwork-fixture.cover");
    let mut bytes = Cursor::new(vec![2]); // PNG tag in the disk artwork format.
    bytes.set_position(1);
    image::RgbImage::from_pixel(64, 64, image::Rgb([209, 86, 42]))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    fs::write(&fixture, bytes.into_inner()).unwrap();
    for cover in [PLAYLIST_COVER, AHEAD_COVER, BEHIND_COVER, FAR_COVER] {
        artwork::seed_cover_for_capture(&profile, cover, &fixture).unwrap();
    }
    let store = Store::open_default().unwrap();
    store
        .connect_account(
            &Session {
                provider: PROVIDER,
                profile: Profile {
                    id: PROFILE.into(),
                    name: "Playlist viewport fixture".into(),
                },
                credentials: Credentials::YandexMusic {
                    access_token: "fixture-playlist-viewport".into(),
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

    let scope = PlaylistScope::open(&playlist_key()).unwrap();
    let tracks = (0..TRACKS)
        .map(|index| {
            let id = (index + 1_000).to_string();
            PlaylistTrack {
                title: format!("Visible row {:02}", index + 1),
                id: id.clone(),
                artist: "Fixture artist".into(),
                artists: Vec::new(),
                artist_credits: Vec::new(),
                album: Some(format!("Album {}", index + 1)),
                album_ref: ObjectRef::native(
                    PROVIDER,
                    ObjectKind::Album,
                    (ALBUM_BASE + index).to_string(),
                ),
                artwork: match index {
                    55 | 56 => Some(AHEAD_COVER.into()),
                    20 | 21 => Some(BEHIND_COVER.into()),
                    90 => Some(FAR_COVER.into()),
                    _ => None,
                },
                added: None,
                duration: Some(184),
                media: Some(Media::Provider {
                    provider: PROVIDER,
                    id,
                }),
            }
        })
        .collect();
    assert!(
        scope
            .save(&Playlist {
                name: "Viewport playlist".into(),
                owner: None,
                artwork: Some(PLAYLIST_COVER.into()),
                link: None,
                tracks
            })
            .unwrap()
    );

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    test_server::use_loopback(listener.local_addr().unwrap());
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&stopped);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let received = Arc::clone(&requests);
    let server = thread::spawn(move || serve(listener, &stop, &received));
    let result = Arc::new(Mutex::new(None));
    let completion = Arc::clone(&result);
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let checked = check(cx, &evidence, &requests).await;
            *completion.lock().unwrap() = Some(checked);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    stopped.store(true, Ordering::Relaxed);
    server.join().unwrap();
    let _ = store.disconnect_account(PROVIDER, PROFILE, || Some(()));
    result.lock().unwrap().take().unwrap().unwrap();
}

fn playlist_key() -> PlaylistKey {
    PlaylistKey::Source {
        provider: PROVIDER,
        profile: Some(PROFILE.into()),
        id: PLAYLIST.into(),
    }
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
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let album = line
            .split_whitespace()
            .nth(1)
            .and_then(|path| path.strip_prefix("/albums/"))
            .and_then(|path| path.strip_suffix("/with-tracks"))
            .and_then(|id| id.parse::<usize>().ok());
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                break;
            }
        }
        let (status, body) = if let Some(id) = album {
            requests.lock().unwrap().push(id);
            (
                "200 OK",
                json!({"result":{"id":id,"title":format!("Fixture album {id}"),"artists":[]}})
                    .to_string(),
            )
        } else {
            ("404 Not Found", "{}".into())
        };
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    }
}

async fn check(
    cx: &mut AsyncApp,
    evidence: &Path,
    requests: &Mutex<Vec<usize>>,
) -> gpui::Result<()> {
    let view = cx.update(|cx| {
        open_capture_window(
            cx,
            CaptureLayout {
                width: 800.,
                height: 400.,
                reduced_motion: true,
            },
        )
    })?;
    view.update(cx, |view, window, cx| {
        view.open_saved_playlist_for_capture(
            PROVIDER,
            PLAYLIST.into(),
            "Viewport playlist".into(),
            window,
            cx,
        )
    })??;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if view.update(cx, |view, window, _| {
            view.playlist_status_for_capture() == Some(Ok(TRACKS))
                && window.viewport_size().width == px(800.)
        })? {
            break;
        }
        wait(cx, deadline, "cached playlist and compact native viewport").await;
    }
    let (first_tree, first_visible) = frame(view, cx, evidence, "before-scroll")?;
    let (first, _) = wait_requests(view, cx, requests, false).await?;
    assert_visible_matches(&first_tree, &first_visible, &first);
    assert!(first.contains(&ALBUM_BASE));
    assert!(!first.contains(&(ALBUM_BASE + 48)));
    assert!(!cover_ready(view, cx, AHEAD_COVER)?);

    view.update(cx, |view, _, cx| view.playlist_scroll_for_capture(48, cx))?;
    let (next_tree, next_visible) = frame(view, cx, evidence, "after-scroll")?;
    assert!(view.update(cx, |view, _, _| {
        view.music.prefetch.viewport.visible().is_empty()
    })?);
    let (next, ahead) = wait_requests(view, cx, requests, true).await?;
    assert_visible_matches(&next_tree, &next_visible, &next);
    assert!(
        next.iter()
            .all(|id| (ALBUM_BASE + 48..ALBUM_BASE + 58).contains(id))
    );
    assert!(next.iter().all(|id| !first.contains(id)));
    assert!(ahead.iter().all(|id| id > next.iter().max().unwrap()));
    wait_for_cover(view, cx, AHEAD_COVER).await?;
    assert!(!cover_ready(view, cx, BEHIND_COVER)?);

    let (track, thumb, _) = view
        .update(cx, |view, window, _| {
            view.playlist_scrollbar_for_capture(window)
        })?
        .unwrap();
    let x = track.center().x;
    let target = point(x, track.top() + track.size.height * 0.28);
    let origin = point(x, thumb);
    dispatch(
        view,
        cx,
        MouseDownEvent {
            button: MouseButton::Left,
            position: origin,
            click_count: 1,
            ..Default::default()
        }
        .to_platform_input(),
    )?;
    dispatch(
        view,
        cx,
        MouseMoveEvent {
            position: target,
            pressed_button: Some(MouseButton::Left),
            ..Default::default()
        }
        .to_platform_input(),
    )?;
    dispatch(
        view,
        cx,
        MouseUpEvent {
            button: MouseButton::Left,
            position: target,
            click_count: 1,
            ..Default::default()
        }
        .to_platform_input(),
    )?;
    let (drag_tree, drag_rows) = frame(view, cx, evidence, "after-thumb-drag")?;
    let (back, behind) = wait_requests(view, cx, requests, true).await?;
    assert_visible_matches(&drag_tree, &drag_rows, &back);
    assert!(back.iter().all(|id| id < next.iter().min().unwrap()));
    assert!(behind.iter().all(|id| id < back.iter().min().unwrap()));
    wait_for_cover(view, cx, BEHIND_COVER).await?;
    assert!(!cover_ready(view, cx, FAR_COVER)?);

    let requested = requests.lock().unwrap().clone();
    let expected = [&first, &next, &ahead, &back, &behind]
        .into_iter()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(requested, expected);

    view.update(cx, |view, window, cx| {
        view.show_music_page(Page::History, window, cx)
    })?;
    assert!(view.update(cx, |view, _, _| {
        view.music.prefetch.viewport.visible().is_empty()
    })?);
    let report = format!(
        "800×400 native virtual playlist (96 rows)\nBefore scrolling: rendered {first_visible:?}; candidates {first:?}\nAfter scrolling: rendered {next_visible:?}; candidates {next:?}; ahead {ahead:?}\nAfter thumb drag: rendered {drag_rows:?}; candidates {back:?}; behind {behind:?}\nYandex HTTP album IDs: {requested:?}\nNeighbor artwork decoded in each direction from the disk cache; distant artwork stayed cold.\nLeaving the route retired all candidates.\n"
    );
    fs::write(evidence.join("viewport.txt"), &report)?;
    print!("{report}");
    Ok(())
}

fn assert_visible_matches(tree: &Value, rendered: &[usize], candidates: &[usize]) {
    assert_eq!(
        tree["frame"]["viewport_size"],
        json!({"width":800.,"height":400.})
    );
    assert!(!candidates.is_empty() && candidates.len() <= MAX_VISIBLE_TARGETS);
    let nodes = tree["nodes"].as_object().unwrap();
    for candidate in candidates {
        let index = candidate - ALBUM_BASE;
        assert!(
            rendered.contains(&index),
            "candidate off the native list viewport: {index}"
        );
        let row = format!(r#"NamedInteger("playlist-row", {index})"#);
        assert!(
            nodes
                .values()
                .any(|node| node["element_id"] == row && node["aria"]["role"] == "ListItem"),
            "candidate lacks a rendered row: {index}"
        );
    }
}

async fn wait_requests(
    view: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    requests: &Mutex<Vec<usize>>,
    moved: bool,
) -> gpui::Result<(Vec<usize>, Vec<usize>)> {
    let deadline = Instant::now() + Duration::from_secs(7);
    loop {
        let (visible, nearby) = view.update(cx, |view, _, _| {
            let scheduler = &view.music.prefetch;
            let id = |key: &Key| {
                assert_eq!(
                    (key.object.provider, key.object.kind, key.profile.as_deref()),
                    (PROVIDER, ObjectKind::Album, Some(PROFILE))
                );
                key.object.id.parse::<usize>().unwrap()
            };
            (
                scheduler
                    .viewport
                    .visible()
                    .iter()
                    .map(id)
                    .collect::<Vec<_>>(),
                scheduler
                    .page
                    .iter()
                    .map(|candidate| id(&candidate.key))
                    .collect::<Vec<_>>(),
            )
        })?;
        if !visible.is_empty()
            && nearby.len() == if moved { 2 } else { 0 }
            && visible
                .iter()
                .chain(&nearby)
                .all(|id| requests.lock().unwrap().contains(id))
        {
            return Ok((visible, nearby));
        }
        wait(cx, deadline, "visible scheduler candidates and actual HTTP").await;
    }
}

fn dispatch(
    view: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    event: PlatformInput,
) -> gpui::Result<()> {
    AnyWindowHandle::from(view).update(cx, |_, window, cx| {
        window.dispatch_event(event, cx);
    })
}

fn cover_ready(view: WindowHandle<MainView>, cx: &mut AsyncApp, url: &str) -> gpui::Result<bool> {
    view.update(cx, |_, _, cx| artwork::decoded_for_capture(url, cx))
}

async fn wait_for_cover(
    view: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    url: &str,
) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !cover_ready(view, cx, url)? {
        wait(cx, deadline, "nearby artwork to decode from the disk cache").await;
    }
    Ok(())
}

async fn wait(cx: &AsyncApp, deadline: Instant, step: &str) {
    assert!(Instant::now() < deadline, "timed out waiting for {step}");
    cx.background_executor()
        .timer(Duration::from_millis(10))
        .await;
}

fn frame(
    view: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    evidence: &Path,
    name: &str,
) -> gpui::Result<(Value, Vec<usize>)> {
    let (tree, image, rows) =
        AnyWindowHandle::from(view).update(cx, |_, window, cx| -> gpui::Result<_> {
            let handle = window.window_handle().map_err(io::Error::other)?;
            let RawWindowHandle::Win32(handle) = handle.as_raw() else {
                return Err(io::Error::other("expected Win32").into());
            };
            unsafe { SendMessageW(handle.hwnd.get() as _, WM_GETOBJECT, 0, -25) };
            window.draw(cx).clear(cx);
            let tree = window
                .debug_a11y_tree_json()
                .ok_or_else(|| io::Error::other("missing accessibility tree"))?;
            let key = playlist_key();
            let rows = (0..TRACKS)
                .filter(|index| {
                    window
                        .rendered_span(crate::ui::panel::playlist_track_artwork_span(&key, *index))
                        .is_some()
                })
                .collect::<Vec<_>>();
            Ok((tree, window.render_to_image()?, rows))
        })??;
    image.save(evidence.join(format!("{name}.png")))?;
    fs::write(evidence.join(format!("{name}-a11y.json")), &tree)?;
    Ok((serde_json::from_str(&tree)?, rows))
}
