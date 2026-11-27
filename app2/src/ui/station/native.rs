mod wire;

use std::{
    env, fs, io,
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, InputEvent, KeyUpEvent, Keystroke, WindowHandle};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{Value, json};
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETOBJECT};

use super::*;
use crate::{
    playlist::{Media, PlaylistTrack},
    providers::{Capabilities, Credentials, Profile, Session, yandex::test_server},
    state::{ActiveSession, HomeContent, HomeView, Intent, Screen},
    storage::Store,
    ui::{CaptureLayout, application, open_capture_window},
};
use wire::{Requests, serve};

const TEST: &str = "ui::station::native::a_delayed_station_cannot_replace_the_new_manual_queue";
const CHILD: &str = "MEDIADEKA_NATIVE_STATION_CHILD";
const EVIDENCE: &str = "MEDIADEKA_NATIVE_STATION_EVIDENCE";
const PROFILE: &str = "94820017";
const FIRST: usize = 89_000_023;
const NEXT: usize = 89_000_024;
const STATION_NAME: &str = "Станция · первая запись";

#[test]
#[ignore = "requires Windows, a native GPUI window, libmpv and a local FLAC fixture"]
fn a_delayed_station_cannot_replace_the_new_manual_queue() {
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

    let evidence = PathBuf::from(env::var_os(EVIDENCE).expect("acceptance output"));
    let media = PathBuf::from(env::var_os("MEDIADEKA_AUDIO_FIXTURE").expect("local FLAC fixture"));
    let audio = fs::read(&media).unwrap();
    assert!(audio.starts_with(b"fLaC"));
    fs::create_dir_all(&evidence).unwrap();
    let store = Store::open_default().unwrap();
    let local = store
        .create_local_playlist("Ручная очередь", std::slice::from_ref(&media))
        .unwrap();
    let key = PlaylistKey::Local(local);
    let session = Session {
        provider: ProviderId::YandexMusic,
        profile: Profile {
            id: PROFILE.into(),
            name: "Station acceptance".into(),
        },
        credentials: Credentials::YandexMusic {
            access_token: "fixture-native-station".into(),
            refresh_token: None,
            access_expires_at: None,
        },
        capabilities: Capabilities {
            home: true,
            profile_selection: false,
            device_code: true,
        },
    };
    assert!(
        store
            .connect_account(&session, || Some(()), || {})
            .unwrap()
            .is_some()
    );
    let accounts = store.accounts().unwrap();
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    server.set_nonblocking(true).unwrap();
    test_server::use_loopback(server.local_addr().unwrap());
    let requests = Arc::new(Requests::default());
    let serving = requests.clone();
    let network = thread::spawn(move || serve(server, &audio, &serving));
    let result = Arc::new(Mutex::new(None));
    let outcome = result.clone();
    let observed = requests.clone();
    application().run(move |cx| {
        let window = open_capture_window(
            cx,
            CaptureLayout {
                width: 800.,
                height: 400.,
                reduced_motion: true,
            },
        )
        .unwrap();
        cx.spawn(async move |cx| {
            let checked = check(window, cx, &evidence, &observed, key, media, accounts).await;
            *outcome.lock().unwrap() = Some(checked);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    requests.stop.store(true, Ordering::Relaxed);
    requests.release_first.store(true, Ordering::Relaxed);
    network.join().unwrap();
    let _ = store.disconnect_account(ProviderId::YandexMusic, PROFILE, || Some(()));
    result.lock().unwrap().take().unwrap().unwrap();
}

fn manual(key: &PlaylistKey, media: &Path, track: usize) -> QueueEntry {
    QueueEntry {
        playlist: key.clone(),
        playlist_revision: None,
        recommendations: Vec::new(),
        index: track,
        track: PlaylistTrack {
            id: format!("manual-{track}"),
            title: format!("Ручная запись {track}"),
            artist: "Mediadeka".into(),
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: None,
            artwork: None,
            added: None,
            duration: None,
            media: Some(Media::File(media.to_owned())),
        },
    }
}

async fn check(
    view: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    evidence: &Path,
    requests: &Requests,
    key: PlaylistKey,
    audio: PathBuf,
    accounts: Vec<crate::storage::AccountSummary>,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(view);
    raw.update(cx, |_, window, _| -> gpui::Result<()> {
        let handle = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(io::Error::other("expected Win32 window").into());
        };
        unsafe { SendMessageW(handle.hwnd.get() as _, WM_GETOBJECT, 0, -25) };
        Ok(())
    })??;
    view.update(cx, |view, window, cx| {
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
        view.open_music_target(my_wave_target(Some(PROFILE.into())), window, cx);
    })?;
    play_manual(view, cx, manual(&key, &audio, 1))?;
    wait(cx, "first manual recording", |cx| {
        playing(view, cx, "manual-1")
    })
    .await?;
    capture(raw, cx, evidence, "ready")?;
    activate(raw, cx, "station-play")?;
    wait(cx, "held request", |_| {
        Ok(requests.starts.load(Ordering::Relaxed) == 1)
    })
    .await?;
    capture(raw, cx, evidence, "pending")?;

    play_manual(view, cx, manual(&key, &audio, 2))?;
    wait(cx, "second recording in the same playlist", |cx| {
        playing(view, cx, "manual-2")
    })
    .await?;
    let immediate = capture(raw, cx, evidence, "manual-while-held")?;
    requests.release_first.store(true, Ordering::Relaxed);
    wait(cx, "the fixture answered the first request", |_| {
        Ok(requests.first_replied.load(Ordering::Relaxed))
    })
    .await?;
    cx.background_executor()
        .timer(Duration::from_millis(600))
        .await;
    let after = capture(raw, cx, evidence, "after-late-response")?;
    let kept = view.update(cx, |view, _, cx| {
        let player = view.player.read(cx);
        view.station.starting.is_none()
            && view.station.active.is_none()
            && player
                .current()
                .is_some_and(|entry| entry.playlist == key && entry.track.id == "manual-2")
            && player.position() > 0.5
    })?;
    let restarted_button = |tree: &Value| {
        tree["nodes"].as_object().is_some_and(|nodes| {
            nodes.values().any(|node| {
                node["element_id"] == r#"Name("station-play")"#
                    && node["aria"]["label"] == "Слушать"
            })
        })
    };
    if !kept
        || requests.feedback.load(Ordering::Relaxed) != 0
        || !restarted_button(&immediate)
        || !restarted_button(&after)
    {
        return Err(io::Error::other(format!(
            "Delayed station won or stayed pending after the manual replacement: kept={kept}, feedback={}, ready={}→{}",
            requests.feedback.load(Ordering::Relaxed),
            restarted_button(&immediate),
            restarted_button(&after),
        )).into());
    }

    activate(raw, cx, "station-play")?;
    let first = FIRST.to_string();
    if let Err(error) = wait(cx, "restarted station plays its HTTP stream", |cx| {
        playing(view, cx, &first)
    })
    .await
    {
        capture(raw, cx, evidence, "restart-failure")?;
        let state = view.update(cx, |view, _, cx| {
            let bar = view.player.read(cx);
            json!({"id":bar.current().map(|entry| entry.track.id.as_str()),
                "status":format!("{:?}",bar.snapshot().status),"position":bar.position(),
                "station":view.station_semantics_for_capture()})
        })?;
        return Err(io::Error::other(format!(
            "{error}; state={state}, starts={}, feedback={}, refills={}, media={}",
            requests.starts.load(Ordering::Relaxed),
            requests.feedback.load(Ordering::Relaxed),
            requests.refills.load(Ordering::Relaxed),
            requests.media.load(Ordering::Relaxed),
        ))
        .into());
    }
    wait(cx, "station refills the queue", |cx| {
        view.update(cx, |view, _, cx| {
            view.player
                .read(cx)
                .upcoming()
                .any(|entry| entry.track.id == NEXT.to_string())
        })
    })
    .await?;
    capture(raw, cx, evidence, "station-restarted")?;
    let playback = view.update(cx, |view, _, cx| {
        let player = view.player.read(cx);
        json!({"current":player.current().map(|entry| entry.track.title.as_str()),
            "position_seconds":player.position(),"upcoming":player.upcoming().count()})
    })?;
    assert_eq!(playback["current"], STATION_NAME);
    assert_eq!(requests.starts.load(Ordering::Relaxed), 2);
    assert_eq!(requests.refills.load(Ordering::Relaxed), 1);
    assert!(requests.media.load(Ordering::Relaxed) >= 1);
    let report = json!({
        "same_playlist_manual_replacement_kept": kept,
        "late_batches_played": 0,
        "station_starts": requests.starts.load(Ordering::Relaxed),
        "feedback_before_restart": 0,
        "station_refills": requests.refills.load(Ordering::Relaxed),
        "media_requests": requests.media.load(Ordering::Relaxed),
        "restarted_station": playback,
    });
    fs::write(
        evidence.join("station-queue.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{report}");
    Ok(())
}

fn play_manual(
    view: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    entry: QueueEntry,
) -> gpui::Result<()> {
    view.update(cx, |view, _, cx| {
        view.player.update(cx, |bar, cx| {
            bar.command(cx, |playback| playback.play(vec![entry], 0))
        });
    })
}

fn playing(view: WindowHandle<MainView>, cx: &mut AsyncApp, id: &str) -> gpui::Result<bool> {
    view.update(cx, |view, _, cx| {
        let player = view.player.read(cx);
        player.current().is_some_and(|entry| entry.track.id == id)
            && player.snapshot().status == Status::Playing
            && player.position() > 0.24
    })
}

async fn wait(
    cx: &mut AsyncApp,
    what: &str,
    mut ready: impl FnMut(&mut AsyncApp) -> gpui::Result<bool>,
) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !ready(cx)? {
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!("timed out waiting for {what}")).into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    Ok(())
}

fn read_tree(window: &gpui::Window) -> gpui::Result<Value> {
    let tree = window
        .debug_a11y_tree_json()
        .ok_or_else(|| io::Error::other("missing accessibility tree"))?;
    Ok(serde_json::from_str(&tree)?)
}

fn capture(
    raw: AnyWindowHandle,
    cx: &mut AsyncApp,
    out: &Path,
    state: &str,
) -> gpui::Result<Value> {
    raw.update(cx, |_, window, cx| {
        window.draw(cx).clear(cx);
        let tree = read_tree(window)?;
        fs::write(
            out.join(format!("{state}-a11y.json")),
            serde_json::to_vec_pretty(&tree)?,
        )?;
        window
            .render_to_image()?
            .save(out.join(format!("{state}.png")))?;
        Ok(tree)
    })?
}

fn activate(raw: AnyWindowHandle, cx: &mut AsyncApp, id: &str) -> gpui::Result<()> {
    let id = format!("Name({id:?})");
    for _ in 0..55 {
        let activated = raw.update(cx, |_, window, cx| -> gpui::Result<bool> {
            window.draw(cx).clear(cx);
            let tree = read_tree(window)?;
            let focus = tree["gpui_focus"].as_str().unwrap_or_default();
            let matched = tree["nodes"][focus]["element_id"] == id;
            let key = if matched { "enter" } else { "tab" };
            let keystroke = Keystroke::parse(key)?;
            window.dispatch_event(
                gpui::KeyDownEvent {
                    keystroke: keystroke.clone(),
                    is_held: false,
                    prefer_character_input: false,
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
            Ok(matched)
        })??;
        if activated {
            return Ok(());
        }
    }
    Err(io::Error::other(format!("keyboard could not activate {id}")).into())
}
