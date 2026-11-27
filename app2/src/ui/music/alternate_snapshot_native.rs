use std::{
    env, fs,
    io::{self, BufRead, BufReader, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, InputEvent, KeyUpEvent, Keystroke, WindowHandle};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETOBJECT};

use crate::{
    music::{ConfirmedLink, MusicPage, ObjectKind, ObjectRef, now, region, same_object},
    playback::{SoundSource, Status},
    playlist::PlaylistTrack,
    providers::{Capabilities, Credentials, Profile, ProviderId, Session},
    state::{ActiveSession, HomeContent, HomeView, Intent, Screen},
    storage::{Account, Store},
    telemetry,
    ui::{
        CaptureLayout, MainView, application, open_capture_window,
        workspace::{MusicTarget, Route},
    },
};

const TEST: &str = "ui::music::alternate_snapshot_native::confirmed_alternate_snapshot_preserves_native_navigation_and_playback";
const CHILD: &str = "MEDIADEKA_ALTERNATE_SNAPSHOT_CHILD";
const EVIDENCE: &str = "MEDIADEKA_ALTERNATE_SNAPSHOT_EVIDENCE";
const SOURCE: &str = "2a1b2c3d4e5f6g7h8i9jkl";
const ALTERNATE: &str = "86182741";
const CURRENT_PROFILE: &str = "69283141";
const HIDDEN_PROFILE: &str = "69283142";
const SPOTIFY_PROFILE: &str = "alternate-album-fixture";
const TITLE: &str = "Один подтверждённый релиз";
const YANDEX_LINK: &str = "Открыть в Mediadeka · Яндекс Музыка";

#[test]
#[ignore = "uses a Windows GPUI window, libmpv, WASAPI, and a real local audio fixture"]
fn confirmed_alternate_snapshot_preserves_native_navigation_and_playback() {
    if env::var_os(CHILD).is_none() {
        let profile = tempfile::tempdir().unwrap();
        assert!(
            Command::new(env::current_exe().unwrap())
                .args(["--exact", TEST, "--ignored", "--nocapture"])
                .env(CHILD, "1")
                .env("MEDIADEKA_PROFILE_DIR", profile.path())
                .env("MEDIADEKA_GUI", "3")
                .env("MEDIADEKA_MUSIC_REGION", "RU")
                .env("HTTP_PROXY", "http://127.0.0.1:1")
                .env("HTTPS_PROXY", "http://127.0.0.1:1")
                .env("NO_PROXY", "127.0.0.1")
                .status()
                .unwrap()
                .success()
        );
        return;
    }
    let audio = PathBuf::from(env::var_os("MEDIADEKA_AUDIO_FIXTURE").expect("audio fixture"));
    let evidence = PathBuf::from(env::var_os(EVIDENCE).expect("acceptance output"));
    fs::create_dir_all(&evidence).unwrap();
    let store = Store::open_default().unwrap();
    let playlist = store
        .create_local_playlist("Offline alternative playback", &[audio])
        .unwrap();
    let (source, alternate) = seed(&store).unwrap();
    let result = Arc::new(Mutex::new(None));
    let outcome = result.clone();
    let fixture = store.clone();
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let finished = async {
                let window = cx.update(|cx| {
                    open_capture_window(
                        cx,
                        CaptureLayout {
                            width: 1152.,
                            height: 736.,
                            reduced_motion: true,
                        },
                    )
                })?;
                check(
                    window, cx, &evidence, &fixture, playlist, &source, &alternate,
                )
                .await
            }
            .await;
            *outcome.lock().unwrap() = Some(finished);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    store
        .disconnect_account(ProviderId::YandexMusic, CURRENT_PROFILE, || Some(()))
        .unwrap();
    store
        .disconnect_account(ProviderId::Spotify, SPOTIFY_PROFILE, || Some(()))
        .unwrap();
    result.lock().unwrap().take().unwrap().unwrap();
}

fn album(provider: ProviderId, id: &str) -> MusicPage {
    let mut page = MusicPage::new(
        ObjectRef::native(provider, ObjectKind::Album, id.into()).unwrap(),
        TITLE.into(),
    );
    page.artists = vec!["Исполнитель стенда".into()];
    page.barcode = Some("0123456789012".into());
    page.tracks.push(PlaylistTrack {
        id: "fixture-track".into(),
        title: "Запись стенда".into(),
        artist: page.artists[0].clone(),
        artists: Vec::new(),
        artist_credits: Vec::new(),
        album: Some(TITLE.into()),
        album_ref: Some(page.reference.clone()),
        artwork: None,
        added: None,
        duration: Some(30),
        media: None,
    });
    page
}

fn seed(store: &Store) -> gpui::Result<(MusicPage, MusicPage)> {
    let source = album(ProviderId::Spotify, SOURCE);
    let alternate = album(ProviderId::YandexMusic, ALTERNATE);
    let reason = same_object(&source, &alternate).unwrap();
    assert_eq!(reason, "UPC/EAN и треклист");
    let references = [source.reference.clone(), alternate.reference.clone()];
    let timestamp = now();
    let links = references.each_ref().map(|object| ConfirmedLink {
        platform: object.provider.as_str().into(),
        id: object.id.clone(),
        url: object.url().unwrap(),
        evidence: reason.into(),
        checked_at: timestamp,
    });
    let region = region();
    let lease = timestamp + 60;
    assert!(store.claim_music_match(&region, &source.reference, timestamp, lease)?);
    store.save_music_match(
        &region,
        &references,
        lease,
        i64::MAX as u64,
        &json!({"links":links}),
    )?;
    store.remember_music_aliases(&region, &source.reference, &links)?;
    assert!(store.save_music_snapshot(
        ProviderId::YandexMusic,
        None,
        &region,
        &format!("album:{ALTERNATE}"),
        &alternate
    )?);
    Ok((source, alternate))
}

async fn check(
    main: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    evidence: &Path,
    store: &Store,
    playlist: i64,
    source: &MusicPage,
    alternate: &MusicPage,
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
        let startup = view.state.begin(Intent::Startup);
        assert!(view.state.finish(
            &startup,
            Screen::Home(HomeView {
                session: ActiveSession::Guest,
                content: HomeContent::Loading,
                accounts: Vec::new(),
                switch_issue: None,
            })
        ));
        view.open_local_playlist_for_capture(playlist, window, cx);
    })?;
    wait(cx, |cx| {
        Ok(main.update(cx, |view, _, _| view.playlist_status_for_capture())? == Some(Ok(1)))
    })
    .await?;
    press(raw, cx, "playlist-play", |node| {
        named(node, "playlist-play")
    })?;
    let before = loop_audio(main, cx, 0.05).await?;
    let network_before = telemetry::received().api_requests;
    let volume_before = volume(&frame(raw, cx, None)?);
    assert!(volume_before.is_number());

    main.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(source.url.as_deref().unwrap(), window, cx)
    })?;
    wait_page(main, cx, &source.reference, &alternate.reference, None).await?;
    wait(cx, |cx| {
        Ok(nodes(&frame(raw, cx, None)?).any(is_yandex_link))
    })
    .await?;
    let visible = capture(raw, cx, evidence, "requested-spotify-alternate-yandex")?;
    assert!(
        nodes(&visible)
            .any(|node| named(node, "music-object-title") && node["aria"]["label"] == TITLE)
    );
    press(raw, cx, YANDEX_LINK, is_yandex_link)?;
    wait_page(main, cx, &alternate.reference, &alternate.reference, None).await?;
    capture(raw, cx, evidence, "native-yandex-source-chip")?;
    history(raw, cx, "workspace-back")?;
    wait_page(main, cx, &source.reference, &alternate.reference, None).await?;
    capture(raw, cx, evidence, "back-restores-spotify-route")?;
    history(raw, cx, "workspace-forward")?;
    wait_page(main, cx, &alternate.reference, &alternate.reference, None).await?;
    capture(raw, cx, evidence, "forward-restores-yandex-route")?;

    store.clear_catalog_snapshots()?;
    let session = Session {
        provider: ProviderId::YandexMusic,
        profile: Profile {
            id: CURRENT_PROFILE.into(),
            name: "Active fixture".into(),
        },
        credentials: Credentials::YandexMusic {
            access_token: "synthetic-cache-profile".into(),
            refresh_token: None,
            access_expires_at: None,
        },
        capabilities: Capabilities {
            home: true,
            profile_selection: false,
            device_code: true,
        },
    };
    store.connect_account(&session, || Some(()), || {})?;
    let scope = store.preferred_account(ProviderId::YandexMusic)?.unwrap();
    let mut hidden = alternate.clone();
    hidden.title = "Приватная карточка другого профиля".into();
    let slot = format!("album:{ALTERNATE}");
    let payload = serde_json::to_string(&hidden)?;
    // Model an existing snapshot of a previous, unselected profile in the same database.
    Connection::open(store.directory().join("mediadeka.sqlite3"))?.execute(
        "INSERT INTO music_snapshot(provider,profile_id,region,slot,payload,saved_at_ms,encoded_bytes)
         VALUES ('yandex-music',?1,?2,?3,?4,unixepoch()*1000,length(CAST(?4 AS BLOB)))",
        params![HIDDEN_PROFILE, region(), slot, payload],
    )?;
    history(raw, cx, "workspace-back")?;
    wait(cx, |cx| {
        main.update(cx, |view, _, _| {
            view.music.object.as_ref().is_some_and(|detail| {
                detail.target.object == source.reference
                    && detail.error.is_some()
                    && detail.data.is_none()
                    && view.music.request.is_none()
            })
        })
    })
    .await?;
    let hidden_frame = capture(raw, cx, evidence, "unselected-profile-not-shown")?;
    assert!(!nodes(&hidden_frame).any(|node| node["aria"]["label"] == hidden.title));
    assert!(store.save_music_snapshot(
        ProviderId::YandexMusic,
        None,
        &region(),
        &slot,
        alternate
    )?);
    main.update(cx, |view, _, cx| {
        let Route::Object(target) = view.workspace.routes.current() else {
            panic!("expected requested object route");
        };
        view.load_music_object(target.clone(), cx);
    })?;
    wait_page(main, cx, &source.reference, &alternate.reference, None).await?;
    capture(raw, cx, evidence, "public-alternate-with-active-profile")?;

    assert!(store.save_music_snapshot(
        ProviderId::YandexMusic,
        Some(&scope),
        &region(),
        &slot,
        alternate
    )?);
    history(raw, cx, "workspace-forward")?;
    wait_page(
        main,
        cx,
        &alternate.reference,
        &alternate.reference,
        Some(CURRENT_PROFILE),
    )
    .await?;
    history(raw, cx, "workspace-back")?;
    wait_page(
        main,
        cx,
        &source.reference,
        &alternate.reference,
        Some(CURRENT_PROFILE),
    )
    .await?;
    let scoped = capture(raw, cx, evidence, "confirmed-active-profile")?;
    assert_eq!(volume_before, volume(&scoped));
    let requests = telemetry::received().api_requests - network_before;
    assert_eq!(requests, 0);
    check_revoked_source(main, cx, evidence, store, source, alternate, &scope).await?;
    let after = loop_audio(main, cx, before.2 + 0.20).await?;
    assert_eq!((before.0, before.1), (after.0, after.1));
    fs::write(
        evidence.join("offline-alternate.json"),
        serde_json::to_vec_pretty(&json!({
            "requested":source.reference,"alternate":alternate.reference,"catalog_requests":requests,
            "local_audio_before_seconds":before.2,"local_audio_after_seconds":after.2,
            "queue_revision":after.0,"queue_length":after.1,"volume":volume_before,
            "unselected_profile_hidden":true,"active_snapshot_profile":CURRENT_PROFILE,
            "public_alternate_allowed_with_active_profile":true,
            "reverse_lookup_preserved_after_source_revocation":true,
            "previously_revoked_explicit_source_open_blocked":true
        }))?,
    )?;
    Ok(())
}

async fn wait_page(
    main: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    requested: &ObjectRef,
    metadata: &ObjectRef,
    profile: Option<&str>,
) -> gpui::Result<()> {
    wait(cx, |cx| main.update(cx, |view, _, _| {
        matches!(view.workspace.routes.current(), Route::Object(target) if &target.object == requested && target.profile.is_none())
            && view.music.object.as_ref().and_then(|detail|detail.data.as_ref()).is_some_and(|page|
                &page.page.reference == metadata && page.profile.as_deref() == profile)
    })).await
}

async fn check_revoked_source(
    main: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    evidence: &Path,
    store: &Store,
    spotify: &MusicPage,
    yandex: &MusicPage,
    yandex_scope: &Account,
) -> gpui::Result<()> {
    let session = Session {
        provider: ProviderId::Spotify,
        profile: Profile {
            id: SPOTIFY_PROFILE.into(),
            name: "Confirmed alternate fixture".into(),
        },
        credentials: Credentials::Spotify {
            client_id: "offline-fixture".into(),
            access_token: "synthetic-cache-profile".into(),
            refresh_token: "synthetic-cache-profile".into(),
            access_expires_at: None,
            librespot: None,
        },
        capabilities: Capabilities {
            home: true,
            profile_selection: false,
            device_code: false,
        },
    };
    assert!(
        store
            .connect_account(&session, || Some(()), || {})?
            .is_some()
    );
    store.clear_catalog_snapshots()?;
    let scope = store.preferred_account(ProviderId::Spotify)?.unwrap();
    assert!(store.save_music_snapshot(
        ProviderId::Spotify,
        Some(&scope),
        &region(),
        &format!("album:{SOURCE}"),
        spotify,
    )?);
    assert!(!store.account_revoked(yandex_scope)?);
    let (arrived, release, server) = hold_yandex_response()?;
    let requests = telemetry::received().api_requests;
    let target = MusicTarget {
        object: yandex.reference.clone(),
        profile: Some(CURRENT_PROFILE.into()),
        title: "Исходная карточка".into(),
        artwork: None,
    };
    main.update(cx, |view, window, cx| {
        view.open_music_target(target.clone(), window, cx)
    })?;
    wait(cx, |cx| {
        main.update(cx, |view, _, _| {
            view.music.object.as_ref().is_some_and(|detail| {
                detail.target == target
                    && detail.data.as_ref().is_some_and(|data| {
                        data.page.reference == spotify.reference
                            && data.profile.as_deref() == Some(SPOTIFY_PROFILE)
                    })
            })
        })
    })
    .await?;
    wait(cx, |_| Ok(arrived.try_recv().is_ok())).await?;
    let raw = AnyWindowHandle::from(main);
    capture(raw, cx, evidence, "alternate-before-source-revoked")?;
    assert!(store.forget_music_account(yandex_scope)?);
    release.send(())?;
    wait(cx, |cx| {
        main.update(cx, |view, _, _| {
            view.music.object.as_ref().is_some_and(|detail| detail.target == target
                && detail.error.as_ref().is_some_and(|failure| failure.needs_sign_in)
                && detail.data.as_ref().is_some_and(|data| data.page.reference == spotify.reference && data.profile.as_deref() == Some(SPOTIFY_PROFILE))
                && detail.links.iter().all(|link| link.platform != ProviderId::YandexMusic.as_str())
                && matches!(view.workspace.routes.current(), Route::Object(current) if current == &target))
        })
    }).await?;
    server.join().unwrap()?;
    assert_eq!(telemetry::received().api_requests - requests, 1);
    let after = capture(raw, cx, evidence, "alternate-remains-after-source-revoked")?;
    assert!(
        nodes(&after)
            .any(|node| named(node, "music-object-title") && node["aria"]["label"] == TITLE)
    );
    assert!(nodes(&after).any(|node| {
        named(node, "music-object-connect")
            && node["aria"]["role"] == "Button"
            && node["aria"]["label"] == "Подключить Яндекс Музыку"
            && node["aria"]["description"] == "Подключение изменилось во время загрузки."
    }));
    assert!(!nodes(&after).any(is_yandex_link));
    wait(cx, |cx| {
        main.update(cx, |view, _, _| view.music.request.is_none())
    })
    .await?;
    main.update(cx, |view, _, cx| view.load_music_object(target.clone(), cx))?;
    wait(cx, |cx| {
        main.update(cx, |view, _, _| {
            view.music.object.as_ref().is_some_and(|detail| {
                detail.target == target
                    && detail.data.is_none()
                    && detail
                        .error
                        .as_ref()
                        .is_some_and(|failure| failure.needs_sign_in)
                    && view.music.request.is_none()
            })
        })
    })
    .await?;
    capture(
        raw,
        cx,
        evidence,
        "previously-revoked-explicit-source-blocked",
    )?;
    assert_eq!(telemetry::received().api_requests - requests, 1);
    Ok(())
}

type ResponseGate = (
    mpsc::Receiver<()>,
    mpsc::SyncSender<()>,
    thread::JoinHandle<io::Result<()>>,
);

fn hold_yandex_response() -> io::Result<ResponseGate> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    crate::providers::yandex::test_server::use_loopback(listener.local_addr()?);
    let (arrived, request) = mpsc::sync_channel(1);
    let (release, permission) = mpsc::sync_channel(1);
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(error),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if !line.starts_with(&format!("GET /albums/{ALTERNATE}/with-tracks ")) {
            return Err(io::Error::other("unexpected Yandex catalog request"));
        }
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 || line == "\r\n" {
                break;
            }
        }
        arrived.send(()).map_err(io::Error::other)?;
        permission
            .recv_timeout(Duration::from_secs(10))
            .map_err(io::Error::other)?;
        let body = json!({"result":{"id":ALTERNATE,"title":"Исходная карточка после отзыва"}});
        let body = body.to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len(),
        )
    });
    Ok((request, release, server))
}

async fn loop_audio(
    main: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    threshold: f64,
) -> gpui::Result<(u64, usize, f64)> {
    let mut state = None;
    wait(cx, |cx| {
        main.update(cx, |view, _, cx| {
            let player = view.player.read(cx);
            let snapshot = player.snapshot();
            let position = player.position();
            if snapshot.status == Status::Playing
                && snapshot.sound_source == Some(SoundSource::LocalFile)
                && position > threshold
            {
                state = Some((snapshot.queue_revision, snapshot.queue_len, position));
            }
            state.is_some()
        })
    })
    .await?;
    Ok(state.unwrap())
}

fn nodes(tree: &Value) -> impl Iterator<Item = &Value> {
    tree["nodes"]
        .as_object()
        .into_iter()
        .flat_map(|nodes| nodes.values())
}

fn named(node: &Value, id: &str) -> bool {
    node["element_id"] == format!("Name({id:?})")
}

fn is_yandex_link(node: &Value) -> bool {
    node["aria"]["role"] == "Link" && node["aria"]["label"] == YANDEX_LINK
}

fn volume(tree: &Value) -> Value {
    nodes(tree)
        .find(|node| named(node, "player-volume"))
        .map_or(Value::Null, |node| node["aria"]["numeric_value"].clone())
}

async fn wait(
    cx: &mut AsyncApp,
    mut condition: impl FnMut(&mut AsyncApp) -> gpui::Result<bool>,
) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition(cx)? {
        if Instant::now() >= deadline {
            return Err(
                io::Error::other("expected offline alternate route or playback state").into(),
            );
        }
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
    }
    Ok(())
}

fn press(
    raw: AnyWindowHandle,
    cx: &mut AsyncApp,
    description: &str,
    selected: impl Fn(&Value) -> bool,
) -> gpui::Result<()> {
    for _ in 0..66 {
        let tree = frame(raw, cx, None)?;
        let focus = tree["gpui_focus"].as_str().unwrap_or_default();
        let key = if selected(&tree["nodes"][focus]) {
            "enter"
        } else {
            "tab"
        };
        raw.update(cx, |_, window, cx| {
            let stroke = Keystroke::parse(key).unwrap();
            window.dispatch_keystroke(stroke.clone(), cx);
            window.dispatch_event(KeyUpEvent { keystroke: stroke }.to_platform_input(), cx);
        })?;
        if key == "enter" {
            return Ok(());
        }
    }
    Err(io::Error::other(format!("keyboard could not focus {description}")).into())
}

fn history(raw: AnyWindowHandle, cx: &mut AsyncApp, name: &str) -> gpui::Result<()> {
    press(raw, cx, name, |node| named(node, name))
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
        if let Some(path) = output {
            window.render_to_image()?.save(path)?;
            fs::write(path.with_extension("json"), &tree)?;
        }
        Ok(serde_json::from_str(&tree)?)
    })?
}
