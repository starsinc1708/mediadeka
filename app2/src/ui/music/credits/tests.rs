use std::{
    env, fs,
    io::{self, BufRead, BufReader, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    sync::{Barrier, Mutex},
    thread,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{Value, json};
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETOBJECT};

use crate::{
    playback::{QueueEntry, SoundSource},
    playlist::{Media, PlaylistKey},
    providers::genius::test_server,
    storage::GeniusSnapshotKind,
    ui::{CaptureLayout, application, open_capture_window, workspace::LocalTrackTarget},
};

use super::*;

const CHILD: &str = "MEDIADEKA_CREDITS_CHILD";
const EVIDENCE: &str = "MEDIADEKA_CREDITS_EVIDENCE";
const AUDIO: &str = "MEDIADEKA_CREDITS_AUDIO";
const TEST: &str = "ui::music::credits::tests::native_genius_credits_warm_and_follow_the_recording";
const SHARED_TEST: &str =
    "ui::music::credits::tests::genius_credits_and_annotations_share_the_cold_details_flight";

#[test]
fn genius_credits_and_annotations_share_the_cold_details_flight() {
    if env::var_os(CHILD).is_none() {
        let profile = tempfile::tempdir().unwrap();
        let status = Command::new(env::current_exe().unwrap())
            .args([SHARED_TEST, "--exact", "--nocapture"])
            .env(CHILD, "1")
            .env("MEDIADEKA_PROFILE_DIR", profile.path())
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let store = Store::open_default().unwrap();
    store
        .set_genius_token(Some("isolated-genius-flight-fixture"))
        .unwrap();
    let recording = Recording::from_entry(&track(1, "only-metadata.wav".into()));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    test_server::use_loopback(listener.local_addr().unwrap());
    let stop = AtomicBool::new(false);
    let requests = Mutex::new(Vec::new());
    let join_count = crate::telemetry::received().joined_reads;
    let (credits, song) = thread::scope(|scope| {
        scope.spawn(|| serve(listener, &stop, &requests, Some(join_count)));
        let barrier = Arc::new(Barrier::new(2));
        // The fixture withholds the response until the second consumer has joined.
        let left = scope.spawn({
            let barrier = Arc::clone(&barrier);
            let store = &store;
            let recording = &recording;
            move || {
                barrier.wait();
                genius::load_credits(store, recording, &AtomicBool::new(false))
            }
        });
        barrier.wait();
        let song = genius::load(&store, &recording, &AtomicBool::new(false), |_| {});
        let credits = left.join();
        stop.store(true, Ordering::Relaxed);
        (credits.unwrap(), song)
    });
    store.set_genius_token(None).unwrap();
    assert_eq!(
        credits.unwrap().unwrap().writers,
        song.unwrap().unwrap().writers
    );
    assert_eq!(
        *requests.lock().unwrap(),
        ["/search", "/songs/1", "/referents"]
    );
    assert!(crate::telemetry::received().joined_reads > join_count);
}

#[test]
#[ignore = "requires Windows D3D11, libmpv and a 32-second WAV in MEDIADEKA_CREDITS_AUDIO"]
fn native_genius_credits_warm_and_follow_the_recording() {
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
    let audio = PathBuf::from(env::var_os(AUDIO).unwrap());
    assert!(audio.is_file());
    fs::create_dir_all(&evidence).unwrap();
    let store = Store::open_default().unwrap();
    store
        .set_genius_token(Some("isolated-genius-metadata-fixture"))
        .unwrap();
    let first = store.directory().join("original.wav");
    fs::copy(&audio, &first).unwrap();
    fs::write(
        first.with_extension("lrc"),
        "[00:01.00]First line\n[00:08.00]Second line\n",
    )
    .unwrap();
    let original = track(1, first);
    let alternate = store.directory().join("alternate.wav");
    fs::copy(&audio, &alternate).unwrap();
    let live = track(2, alternate);
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let expected = Arc::clone(&requests);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    test_server::use_loopback(listener.local_addr().unwrap());
    let server = thread::spawn(move || serve(listener, &stopped, &expected, None));
    let result = Arc::new(Mutex::new(None));
    let done = Arc::clone(&result);
    let observed = Arc::clone(&requests);
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            *done.lock().unwrap() = Some(check(cx, &evidence, &original, &live, &observed).await);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    store.set_genius_token(None).unwrap();
    result.lock().unwrap().take().unwrap().unwrap();
}

fn track(id: u64, audio: PathBuf) -> QueueEntry {
    QueueEntry {
        playlist: PlaylistKey::Local(1),
        playlist_revision: None,
        index: id as usize - 1,
        track: PlaylistTrack {
            id: id.to_string(),
            title: title(id).into(),
            artist: "Sample Room".into(),
            artists: vec!["Sample Room".into()],
            artist_credits: Vec::new(),
            album: None,
            album_ref: None,
            artwork: None,
            added: None,
            duration: Some(32),
            media: Some(Media::File(audio)),
        },
        recommendations: Vec::new(),
    }
}

fn title(id: u64) -> &'static str {
    if id == 1 {
        "Copper Hour"
    } else {
        "Copper Hour (Live)"
    }
}

fn song(id: u64) -> Value {
    let label = if id == 1 { "Original" } else { "Live" };
    json!({"id":id, "title":title(id), "primary_artist":{"name":"Sample Room"},
        "duration_ms":32_000, "url":format!("https://genius.com/{label}-lyrics"),
        "writer_artists":[{"name":format!("Writer {label}")}],
        "producer_artists":[{"name":format!("Producer {label}")}]})
}

fn serve(
    listener: TcpListener,
    stopped: &AtomicBool,
    requests: &Mutex<Vec<String>>,
    join_before_search_response: Option<u64>,
) {
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
        let mut request = BufReader::new(&stream);
        let mut line = String::new();
        request.read_line(&mut line).unwrap();
        let path = line.split_whitespace().nth(1).unwrap();
        let url = url::Url::parse(&format!("http://localhost{path}")).unwrap();
        let payload = match url.path() {
            "/search" => {
                json!({"hits":[{"type":"song","result":song(1)},{"type":"song","result":song(2)}]})
            }
            "/songs/1" => json!({"song":song(1)}),
            "/songs/2" => json!({"song":song(2)}),
            "/referents" => json!({"referents":[]}),
            path => panic!("Unexpected Genius route {path}"),
        };
        requests.lock().unwrap().push(url.path().to_owned());
        loop {
            line.clear();
            if request.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                break;
            }
        }
        let body = json!({"meta":{"status":200},"response":payload}).to_string();
        if url.path() == "/search"
            && let Some(previous) = join_before_search_response
        {
            let deadline = Instant::now() + Duration::from_secs(8);
            while crate::telemetry::received().joined_reads == previous {
                assert!(
                    Instant::now() < deadline,
                    "second Genius consumer did not join the active details request"
                );
                thread::sleep(Duration::from_millis(4));
            }
        }
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nCache-Control: public, max-age=600\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    }
}

async fn check(
    cx: &mut AsyncApp,
    evidence: &Path,
    original: &QueueEntry,
    live: &QueueEntry,
    requests: &Mutex<Vec<String>>,
) -> gpui::Result<()> {
    let window = cx.update(|cx| {
        open_capture_window(
            cx,
            CaptureLayout {
                width: 1500.,
                height: 900.,
                reduced_motion: true,
            },
        )
    })?;
    window.update(cx, |view, window, cx| {
        view.start_home_skeleton_for_capture(window, cx);
        view.player.update(cx, |bar, cx| {
            bar.command(cx, |playback| playback.play(vec![original.clone()], 0))
        });
    })?;
    wait_for_credits(window, original, cx).await?;
    assert_eq!(*requests.lock().unwrap(), ["/search", "/songs/1"]);
    assert!(window.update(cx, |view, _, cx| {
        view.player.read(cx).current_sound_source() == Some(SoundSource::LocalFile)
            && view.lyrics_state_for_capture(cx)["visible"] == false
    })?);
    let store = Store::open_default()?;
    let epoch = store.genius_session()?.unwrap().epoch;
    let key = serde_json::to_string(&Recording::from_entry(original))?;
    assert!(
        store
            .genius_snapshot::<Value>(epoch, &key, GeniusSnapshotKind::Details)?
            .is_some()
    );
    assert!(
        store
            .genius_snapshot::<Value>(epoch, &key, GeniusSnapshotKind::Annotations)?
            .is_none()
    );

    open(window, original, cx)?;
    let track = frame(window, evidence, "current-credits", cx)?;
    assert!(
        track.contains("music-track-credits")
            && track.contains("Writer Original")
            && track.contains("Producer Original")
    );
    assert_eq!(*requests.lock().unwrap(), ["/search", "/songs/1"]);

    window.update(cx, |view, window, cx| {
        view.show_track_lyrics(original.clone(), window, cx)
    })?;
    let prior = window.update(cx, |view, _, cx| view.player.read(cx).position())?;
    open(window, live, cx)?;
    wait_for_credits(window, live, cx).await?;
    let track = frame(window, evidence, "other-recording-credits-and-lyrics", cx)?;
    assert!(
        track.contains("music-track-credits")
            && track.contains("Writer Live")
            && track.contains("Producer Live")
    );
    let deadline = Instant::now() + Duration::from_secs(6);
    let state = loop {
        let state = window.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
        if state["player_position"].as_f64().unwrap() > prior
            && requests
                .lock()
                .unwrap()
                .iter()
                .any(|path| path == "/referents")
        {
            break state;
        }
        assert!(
            Instant::now() < deadline,
            "audio or explicit annotation did not continue"
        );
        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    };
    assert_eq!(state["current_track"], "1");
    assert_eq!(state["lyrics_track"], "1");
    assert_eq!(state["visible"], true);
    assert_eq!(state["playing"], true);
    assert!(state["player_position"].as_f64().unwrap() > prior);
    let routes = requests.lock().unwrap().clone();
    assert_eq!(routes.iter().filter(|path| *path == "/songs/1").count(), 1);
    assert_eq!(routes.iter().filter(|path| *path == "/songs/2").count(), 1);
    assert_eq!(
        routes.iter().filter(|path| *path == "/referents").count(),
        1
    );

    store.set_genius_token(None)?;
    window.update(cx, |view, _, cx| view.genius_settings_changed(cx))?;
    assert!(!frame(window, evidence, "disconnected-credits", cx)?.contains("music-track-credits"));
    let report = format!(
        "Genius routes: {routes:?}\nOriginal audio and lyrics remained selected at {:.3} s while other credits loaded\n",
        state["player_position"].as_f64().unwrap()
    );
    fs::write(evidence.join("protocol.txt"), &report)?;
    print!("{report}");
    Ok(())
}

fn open(window: WindowHandle<MainView>, entry: &QueueEntry, cx: &mut AsyncApp) -> gpui::Result<()> {
    window.update(cx, |view, window, cx| {
        view.navigate(
            Route::LocalTrack(Box::new(LocalTrackTarget {
                key: entry.playlist.clone(),
                track: entry.track.clone(),
            })),
            window,
            cx,
        )
    })
}

async fn wait_for_credits(
    window: WindowHandle<MainView>,
    entry: &QueueEntry,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let recording = Recording::from_entry(entry);
    while !window.update(cx, |view, _, _| {
        view.music
            .credits
            .0
            .iter()
            .any(|read| read.recording == recording && read.credits.is_some())
    })? {
        assert!(
            Instant::now() < deadline,
            "Genius credits did not arrive for {}",
            entry.track.title
        );
        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    }
    Ok(())
}

fn frame(
    window: WindowHandle<MainView>,
    evidence: &Path,
    name: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<String> {
    AnyWindowHandle::from(window).update(cx, |_, window, cx| {
        let handle = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(io::Error::other("expected Win32").into());
        };
        unsafe { SendMessageW(handle.hwnd.get() as _, WM_GETOBJECT, 0, -25) };
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
