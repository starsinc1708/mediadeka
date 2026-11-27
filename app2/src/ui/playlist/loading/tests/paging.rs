use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use crate::{
    playback::Status,
    providers::{Capabilities, Credentials, Profile, Session},
};

use super::*;

const CHILD: &str = "MEDIADEKA_NATIVE_PLAYLIST_PAGING_CHILD";
const EVIDENCE: &str = "MEDIADEKA_NATIVE_PLAYLIST_PAGING_EVIDENCE";
const COMPACT: &str = "MEDIADEKA_NATIVE_PLAYLIST_PAGING_COMPACT";
const TEST: &str =
    "ui::playlist::loading::tests::paging::native_jellyfin_playlist_shows_received_rows";
const PROFILE: &str = "4444333322221111aaaabbbbccccddee";
const PLAYLIST: &str = "00112233445566778899aabbccddeeff";
const INTERRUPTED: &str = "ffeeccbbaa9988776655443322110066";
const TRACKS: [&str; 3] = [
    "11111111222222223333333344444444",
    "22222222333333334444444455555555",
    "33333333444444445555555566666666",
];

#[test]
#[ignore = "uses native GPUI, libmpv, and a paginated loopback Jellyfin response"]
fn native_jellyfin_playlist_shows_received_rows() {
    if env::var_os(CHILD).is_none() {
        let directory = tempfile::tempdir().unwrap();
        let result = Command::new(env::current_exe().unwrap())
            .args(["--exact", TEST, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("MEDIADEKA_PROFILE_DIR", directory.path())
            .env("MEDIADEKA_GUI", "3")
            .status()
            .unwrap();
        assert!(result.success());
        return;
    }

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let store = Store::open_default().unwrap();
    let mut session = Session {
        provider: ProviderId::Jellyfin,
        profile: Profile {
            id: PROFILE.into(),
            name: "Playlist paging fixture".into(),
        },
        credentials: Credentials::Jellyfin {
            server: format!("http://{}", listener.local_addr().unwrap()),
            device: "playlist-paging-device".into(),
            access_token: "playlist-paging-loopback".into(),
        },
        capabilities: Capabilities {
            home: true,
            profile_selection: false,
            device_code: false,
        },
    };
    store
        .connect_account(&session, || Some(()), || {})
        .unwrap()
        .unwrap();
    let _account = FixtureAccount(&store, PROFILE);
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&stopped);
    let (held, requests) = mpsc::channel();
    let server = thread::spawn(move || serve(listener, &stop, &held));
    let result = Arc::new(Mutex::new(None));
    let completion = Arc::clone(&result);
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            *completion.lock().unwrap() = Some(check_paging(&requests, &mut session, cx).await);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    stopped.store(true, Ordering::Relaxed);
    server.join().unwrap();
    result.lock().unwrap().take().unwrap().unwrap();
}

async fn check_paging(
    requests: &Receiver<Sender<bool>>,
    session: &mut Session,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let mode = env::var(COMPACT).unwrap_or_default();
    let compact = !mode.is_empty();
    let window = cx.update(|cx| {
        open_capture_window(
            cx,
            CaptureLayout {
                width: if compact { 800. } else { 1920. },
                height: if compact { 400. } else { 1080. },
                reduced_motion: true,
            },
        )
    })?;
    if mode == "150" {
        apply_compact_dpi(window, cx)?;
    }
    enable_accessibility(window, cx)?;
    let scope = scope(PLAYLIST)?;
    let started = Instant::now();
    open(window, PLAYLIST, cx)?;
    let final_page = wait_for_page(requests, "cold open", cx).await?;
    prefetch::wait_for_playlist(window, cx, |open| {
        open.load_progress().as_deref() == Some("2 из 3")
    })
    .await?;
    AnyWindowHandle::from(window).update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
    let first_frame = started.elapsed();
    let cold_sample = window
        .update(cx, |view, _, _| view.last_content_opening())?
        .filter(|sample| sample.label() == "плейлиста")
        .ok_or_else(|| {
            io::Error::other("The partial frame did not register as first useful content")
        })?;
    verify_partial_accessibility(window, cx, mode == "150")?;
    let partial = inspect(window, cx)?;
    check(partial.0 == TRACKS[..2], "Incorrect first page IDs")?;
    check(
        scope.saved()?.is_none(),
        "The partial playlist was persisted",
    )?;
    capture(window, cx, "first-page.png")?;

    window.update(cx, |view, _, cx| {
        view.play_playlist_track_for_capture(0, cx)
    })?;
    let deadline = Instant::now() + Duration::from_secs(7);
    let position = loop {
        let (playing, length, position) = window.update(cx, |view, _, cx| {
            (
                matches!(view.player_snapshot.status, Status::Playing),
                view.player_snapshot.queue_len,
                view.player.read(cx).position(),
            )
        })?;
        if playing && length == 2 && position > 0.15 {
            break position;
        }
        check(
            Instant::now() < deadline,
            format!("Partial playback did not start, queue={length}, position={position}"),
        )?;
        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    };
    check(
        scope.saved()?.is_none(),
        "Playback persisted a partial playlist",
    )?;
    capture(window, cx, "first-page-playing.png")?;
    final_page.send(true)?;
    prefetch::wait_for_playlist(window, cx, |open| {
        open.complete_snapshot().is_some() && open.request.is_none()
    })
    .await?;
    let complete = inspect(window, cx)?;
    check(
        complete.0 == TRACKS && complete.1 == partial.1,
        "Completing the page changed a displayed row's revision",
    )?;
    check(
        window.update(cx, |view, _, _| {
            view.player_snapshot
                .highlights(&key(PLAYLIST), complete.1, 0)
                && view.player_snapshot.queue_len == 2
        })?,
        "Completing hydration modified active playback",
    )?;
    check(
        scope
            .saved()?
            .is_some_and(|saved| saved.playlist.tracks.len() == 3),
        "No full snapshot after completion",
    )?;
    AnyWindowHandle::from(window).update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
    let complete_frame = started.elapsed();
    check(
        window.update(cx, |view, _, _| view.last_content_opening())? == Some(cold_sample),
        "The final page replaced the first-content measurement",
    )?;
    capture(window, cx, "complete.png")?;

    window.update(cx, |view, _, cx| view.refresh_playlist(cx))?;
    let final_page = wait_for_page(requests, "forced refresh", cx).await?;
    check(
        inspect(window, cx)? == complete,
        "Refresh replaced the cached full snapshot with a page",
    )?;
    final_page.send(false)?;
    prefetch::wait_for_playlist(window, cx, |open| open.refresh_error.is_some()).await?;
    check(
        inspect(window, cx)? == complete,
        "Unavailable source discarded cached rows",
    )?;
    capture(window, cx, "cached-refresh-error.png")?;

    open(window, INTERRUPTED, cx)?;
    let final_page = wait_for_page(requests, "interrupted open", cx).await?;
    final_page.send(false)?;
    prefetch::wait_for_playlist(window, cx, |open| open.refresh_error.is_some()).await?;
    check(
        inspect(window, cx)?.0 == TRACKS[..2],
        "Unavailable later page discarded valid rows",
    )?;
    check(
        self::scope(INTERRUPTED)?.saved()?.is_none(),
        "An interrupted playlist was persisted",
    )?;
    capture(window, cx, "partial-error.png")?;

    let warm_started = Instant::now();
    open(window, PLAYLIST, cx)?;
    prefetch::wait_for_playlist(window, cx, |open| open.complete_snapshot().is_some()).await?;
    AnyWindowHandle::from(window).update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
    let warm_frame = warm_started.elapsed();
    check(
        inspect(window, cx)? == complete,
        "Warm reopen changed the snapshot revision",
    )?;
    check(
        matches!(requests.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "Warm reopen contacted the provider",
    )?;
    let store = Store::open_default()?;
    let account = store.account(ProviderId::Jellyfin, PROFILE)?.unwrap();
    let Credentials::Jellyfin { access_token, .. } = &mut session.credentials else {
        unreachable!("Jellyfin fixture credentials");
    };
    *access_token = "playlist-paging-reconnected".into();
    check(
        store
            .reconnect_account(session, &account, || Some(()), || {})?
            .is_some(),
        "Replacing the loopback credentials failed",
    )?;
    window.update(cx, |view, window, cx| {
        view.show_music_page(crate::ui::workspace::Page::Home, window, cx);
    })?;
    open(window, PLAYLIST, cx)?;
    prefetch::wait_for_playlist(window, cx, |open| open.complete_snapshot().is_some()).await?;
    let reconnected = inspect(window, cx)?;
    check(
        reconnected.0 == complete.0 && reconnected.1 != complete.1,
        "Replacing credentials reused a resident snapshot from the previous authority",
    )?;
    let account = store.account(ProviderId::Jellyfin, PROFILE)?.unwrap();
    check(
        store.forget_music_account(&account)?,
        "The loopback account was not revoked",
    )?;
    window.update(cx, |view, window, cx| {
        view.show_music_page(crate::ui::workspace::Page::Home, window, cx);
    })?;
    open(window, PLAYLIST, cx)?;
    check(
        window.update(cx, |view, _, _| {
            view.playlist.open.as_ref().unwrap().snapshot().is_none()
        })?,
        "Revoked account exposed its old resident snapshot",
    )?;
    prefetch::wait_for_playlist(
        window,
        cx,
        |open| matches!(&open.content, Content::Error(error) if error.needs_sign_in),
    )
    .await?;
    check(
        matches!(requests.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "The revoked account sent a playlist request",
    )?;
    capture(window, cx, "revoked-reopen.png")?;
    println!(
        "Jellyfin native first submitted frame {:.2} ms, complete {:.2} ms, warm receipt {:.2} ms; playback advanced to {position:.3}s with two rows while page 2 was held; cache, interrupt, and revocation verified",
        first_frame.as_secs_f64() * 1_000.,
        complete_frame.as_secs_f64() * 1_000.,
        warm_frame.as_secs_f64() * 1_000.,
    );
    println!(
        "App recorded first content at {:.2} ms",
        cold_sample.elapsed.as_secs_f64() * 1_000.
    );
    Ok(())
}

pub(super) fn apply_compact_dpi(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClientRect, GetWindowRect, SendMessageW, WM_DPICHANGED,
    };

    let (hwnd, rect) = window.update(cx, |_, window, _| -> gpui::Result<_> {
        let raw = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::Win32(handle) = raw.as_raw() else {
            return Err(io::Error::other("Expected a Win32 capture window").into());
        };
        let hwnd = handle.hwnd.get() as _;
        let (mut rect, mut client) = (Default::default(), Default::default());
        // Both output rectangles are valid for the lifetime of the Win32 calls.
        if unsafe { GetWindowRect(hwnd, &mut rect) == 0 || GetClientRect(hwnd, &mut client) == 0 } {
            return Err(io::Error::last_os_error().into());
        }
        rect.right += 1_200 - (client.right - client.left);
        rect.bottom += 600 - (client.bottom - client.top);
        Ok((hwnd, rect))
    })??;
    // The WM_DPICHANGED handler runs synchronously, after releasing the GPUI view.
    unsafe {
        SendMessageW(
            hwnd,
            WM_DPICHANGED,
            144 | 144 << 16,
            &rect as *const _ as isize,
        )
    };
    Ok(())
}

pub(super) fn enable_accessibility(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETOBJECT};

    let hwnd = window.update(cx, |_, window, _| -> gpui::Result<_> {
        let raw = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::Win32(handle) = raw.as_raw() else {
            return Err(io::Error::other("Expected a Win32 capture window").into());
        };
        Ok(handle.hwnd.get() as _)
    })??;
    const UIA_ROOT: isize = -25;
    // Request the accessibility root while the native capture window is alive.
    unsafe { SendMessageW(hwnd, WM_GETOBJECT, 0, UIA_ROOT) };
    Ok(())
}

fn verify_partial_accessibility(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    scaled: bool,
) -> gpui::Result<()> {
    let tree = window.update(cx, |_, window, _| {
        window
            .debug_a11y_tree_json()
            .ok_or_else(|| io::Error::other("No accessibility snapshot"))
    })??;
    let snapshot: Value = serde_json::from_str(&tree)?;
    let nodes = snapshot["nodes"]
        .as_object()
        .ok_or_else(|| io::Error::other("No accessible nodes"))?;
    check(
        nodes.values().any(|node| {
            node["aria"]["role"] == "Status"
                && node["aria"]["label"]
                    .as_str()
                    .is_some_and(|label| label.starts_with("Загружено 2 из 3"))
        }),
        "The partial status is missing from the accessibility tree",
    )?;
    if scaled {
        check(
            snapshot["frame"]["scale_factor"] == 1.5,
            "The capture did not switch to 150% DPI",
        )?;
    }
    if let Some(path) = env::var_os(EVIDENCE) {
        let directory = PathBuf::from(path);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(directory.join("first-page-a11y.json"), tree)?;
    }
    Ok(())
}

fn key(id: &str) -> PlaylistKey {
    PlaylistKey::Source {
        provider: ProviderId::Jellyfin,
        profile: Some(PROFILE.into()),
        id: id.into(),
    }
}

fn scope(id: &str) -> gpui::Result<PlaylistScope> {
    PlaylistScope::open(&key(id)).map_err(|error| io::Error::other(error.to_string()).into())
}

fn open(window: WindowHandle<MainView>, id: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    window.update(cx, |view, window, cx| {
        view.open_saved_playlist_for_capture(
            ProviderId::Jellyfin,
            id.into(),
            "Jellyfin paging".into(),
            window,
            cx,
        )
    })?
}

fn capture(window: WindowHandle<MainView>, cx: &mut AsyncApp, name: &str) -> gpui::Result<()> {
    if let Some(path) = env::var_os(EVIDENCE) {
        let directory = PathBuf::from(path);
        std::fs::create_dir_all(&directory)?;
        frame(window, cx, &directory.join(name))?;
    }
    Ok(())
}

async fn wait_for_page(
    requests: &Receiver<Sender<bool>>,
    phase: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<Sender<bool>> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match requests.try_recv() {
            Ok(release) => return Ok(release),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(io::Error::other("The loopback fixture stopped").into());
            }
            Err(mpsc::TryRecvError::Empty) => (),
        }
        check(
            Instant::now() < deadline,
            format!("No page 2 request during {phase}"),
        )?;
        cx.background_executor()
            .timer(Duration::from_millis(4))
            .await;
    }
}

fn serve(listener: TcpListener, stop: &AtomicBool, held: &Sender<Sender<bool>>) {
    thread::scope(|workers| {
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    workers.spawn(move || handle(stream, held));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2))
                }
                Err(error) => panic!("Accepting loopback request: {error}"),
            }
        }
    });
}

fn track(index: usize) -> Value {
    let title = ["Morning Light", "Between Stations", "Open Water"][index];
    serde_json::json!({
        "Id": TRACKS[index],
        "Name": title,
        "Type": "Audio",
        "Album": "Jellyfin loopback",
        "Artists": ["Fixture Artist"],
        "RunTimeTicks": 100_000_000
    })
}

fn handle(mut stream: TcpStream, held: &Sender<Sender<bool>>) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(&stream);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let Some(uri) = request.split_whitespace().nth(1) else {
        return Ok(());
    };
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
    }
    if uri.starts_with("/Audio/") {
        return respond(&mut stream, "200 OK", "audio/wav", &wave());
    }
    let body = if uri.starts_with("/Users/Me") {
        serde_json::json!({"Id": PROFILE, "Name": "Fixture"})
    } else if uri.starts_with(&format!("/Items/{PLAYLIST}?"))
        || uri.starts_with(&format!("/Items/{INTERRUPTED}?"))
    {
        serde_json::json!({"Type": "Playlist", "Name": "Jellyfin paging"})
    } else if uri.starts_with("/Playlists/") && uri.contains("StartIndex=2") {
        let (release, wait) = mpsc::channel();
        let sent = held.send(release).is_ok();
        if !sent || !wait.recv_timeout(Duration::from_secs(12)).unwrap_or(false) {
            return respond(
                &mut stream,
                "503 Service Unavailable",
                "application/json",
                b"{}",
            );
        }
        serde_json::json!({"Items": [track(2)], "TotalRecordCount": 3})
    } else if uri.starts_with("/Playlists/") && uri.contains("StartIndex=0") {
        serde_json::json!({"Items": [track(0), track(1)], "TotalRecordCount": 3})
    } else {
        serde_json::json!({"Items": [], "TotalRecordCount": 0})
    };
    respond(
        &mut stream,
        "200 OK",
        "application/json",
        body.to_string().as_bytes(),
    )
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

fn wave() -> Vec<u8> {
    const RATE: u32 = 16_000;
    const SAMPLES: u32 = 10 * RATE;
    let bytes = SAMPLES * 2;
    let mut wav = b"RIFF".to_vec();
    wav.extend((36 + bytes).to_le_bytes());
    wav.extend(b"WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00");
    wav.extend(RATE.to_le_bytes());
    wav.extend((2 * RATE).to_le_bytes());
    wav.extend(b"\x02\x00\x10\x00data");
    wav.extend(bytes.to_le_bytes());
    for sample in 0..SAMPLES {
        let phase = sample as f32 * std::f32::consts::TAU * 330. / RATE as f32;
        wav.extend(((phase.sin() * 4_000.) as i16).to_le_bytes());
    }
    wav
}
