use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    process::Command,
    thread,
};

use crate::{
    playlist::library::prefetch_playlist,
    providers::{Capabilities, Credentials, Profile, Session, yandex::test_server},
};

use super::*;

const TEST: &str =
    "ui::playlist::loading::tests::prefetch::native_playlist_uses_prefetched_snapshot";
const CHILD: &str = "MEDIADEKA_NATIVE_PLAYLIST_CACHE_FIXTURE";
const EVIDENCE: &str = "MEDIADEKA_NATIVE_PLAYLIST_CACHE_EVIDENCE";
const SOURCE: ProviderId = ProviderId::YandexMusic;
const PROFILE: &str = "74368";
const PLAYLIST: &str = "74368:42";

#[test]
#[ignore = "renders a native Fluid Glass window and uses loopback Yandex catalog"]
fn native_playlist_uses_prefetched_snapshot() {
    if env::var_os(CHILD).is_none() {
        let directory = tempfile::tempdir().unwrap();
        let status = Command::new(env::current_exe().unwrap())
            .args(["--exact", TEST, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("MEDIADEKA_PROFILE_DIR", directory.path())
            .env("MEDIADEKA_GUI", "3")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }

    let store = Store::open_default().unwrap();
    store
        .connect_account(
            &Session {
                provider: SOURCE,
                profile: Profile {
                    name: "Playback fixture".into(),
                    id: PROFILE.into(),
                },
                credentials: Credentials::YandexMusic {
                    access_token: "fixture-playlist-cache-access".into(),
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
    struct AccountGuard<'a>(&'a Store);
    impl Drop for AccountGuard<'_> {
        fn drop(&mut self) {
            let _ = self.0.disconnect_account(SOURCE, PROFILE, || Some(()));
        }
    }
    let _account = AccountGuard(&store);

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    test_server::use_loopback(listener.local_addr().unwrap());
    let server = thread::spawn(move || serve_playlist(listener));
    let key = PlaylistKey::Source {
        provider: SOURCE,
        profile: Some(PROFILE.into()),
        id: PLAYLIST.into(),
    };
    let requests = telemetry::received().api_requests;
    let cancelled = AtomicBool::new(false);
    let started = Instant::now();
    prefetch_playlist(&key, &cancelled).unwrap();
    let elapsed = started.elapsed();
    server.join().unwrap();
    prefetch_playlist(&key, &cancelled).unwrap();
    assert_eq!(telemetry::received().api_requests - requests, 1);
    println!(
        "Playlist cold prefetch {:.2} ms, one source request",
        elapsed.as_secs_f64() * 1_000.
    );

    let result = Arc::new(Mutex::new(None));
    let completion = Arc::clone(&result);
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            *completion.lock().unwrap() = Some(capture_cached_playlist(cx).await);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    result.lock().unwrap().take().unwrap().unwrap();
}

fn serve_playlist(listener: TcpListener) {
    let (mut stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(4)))
        .unwrap();
    let mut request = BufReader::new(&stream);
    let mut line = String::new();
    request.read_line(&mut line).unwrap();
    assert!(
        line.starts_with("GET /users/74368/playlists/42 HTTP/"),
        "{line}"
    );
    loop {
        line.clear();
        request.read_line(&mut line).unwrap();
        if line.trim().is_empty() {
            break;
        }
    }
    let body = serde_json::json!({"result": {"title":"Prefetched playlist", "tracks":[
        {"track":{"id":71,"title":"First track","artists":[{"id":81,"name":"First artist"}],"durationMs":120000}},
        {"track":{"id":72,"title":"Second track","artists":[{"id":82,"name":"Second artist"}],"durationMs":135000}},
        {"track":{"id":71,"title":"First track","artists":[{"id":81,"name":"First artist"}],"durationMs":120000}}
    ]}}).to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    ).unwrap();
}

async fn capture_cached_playlist(cx: &mut AsyncApp) -> gpui::Result<()> {
    let window = cx.update(|cx| {
        open_capture_window(
            cx,
            CaptureLayout {
                width: 1500.,
                height: 850.,
                reduced_motion: true,
            },
        )
    })?;
    let raw = AnyWindowHandle::from(window);
    raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
    let started = Instant::now();
    window.update(cx, |view, window, cx| {
        view.open_saved_playlist_for_capture(
            SOURCE,
            PLAYLIST.into(),
            "Prefetched playlist".into(),
            window,
            cx,
        )
    })??;
    wait_for_playlist(window, cx, |open| {
        open.snapshot()
            .is_some_and(|snap| snap.playlist.tracks.len() == 3)
    })
    .await?;
    raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
    println!(
        "Playlist warm open to submitted native frame {:.2} ms",
        started.elapsed().as_secs_f64() * 1_000.
    );
    check(
        telemetry::catalog().fresh == 1,
        "Opening the prefetched playlist missed the cache",
    )?;
    let initial = inspect(window, cx)?;
    check(
        initial.0 == ["71", "72", "71"],
        "The source response was not preserved",
    )?;

    window.update(cx, |view, window, cx| {
        view.select_track(2, window, cx);
        view.show_music_page(crate::ui::workspace::Page::Home, window, cx);
        view.playlist_history_for_capture(false, window, cx);
    })?;
    wait_for_playlist(window, cx, |open| open.request.is_none()).await?;
    check(
        window.update(cx, |view, _, _| {
            view.playlist
                .open
                .as_ref()
                .is_some_and(|open| open.request.is_none() && open.selected == Some(2))
        })?,
        "Back navigation did not restore the second occurrence from validated cache",
    )?;

    window.update(cx, |view, _, cx| view.refresh_playlist(cx))?;
    wait_for_playlist(window, cx, |open| open.refresh_error.is_some()).await?;
    check(
        inspect(window, cx)? == initial,
        "An offline refresh discarded or reordered cached rows",
    )?;
    if let Some(path) = env::var_os(EVIDENCE) {
        let directory = PathBuf::from(path);
        std::fs::create_dir_all(&directory)?;
        frame(
            window,
            cx,
            &directory.join("playlist-prefetched-offline.png"),
        )?;
    }
    Ok(())
}

pub(super) async fn wait_for_playlist(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    ready: impl Fn(&OpenPlaylist) -> bool,
) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if window.update(cx, |view, _, _| {
            view.playlist.open.as_ref().is_some_and(&ready)
        })? {
            return Ok(());
        }
        check(Instant::now() < deadline, "Playlist state did not settle")?;
        cx.background_executor()
            .timer(Duration::from_millis(3))
            .await;
    }
}
