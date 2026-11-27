use std::{
    env, fs,
    io::{self, BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, ScrollDelta, ScrollWheelEvent, WindowHandle, point,
    px,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETOBJECT};

use crate::{
    music::{ObjectKind, ObjectRef, now, region},
    providers::{
        Capabilities, Credentials, Home, HomeItem, HomeSection, Profile, ProviderId, Session,
        yandex::test_server,
    },
    state::{ActiveSession, HomeContent, HomeView, Intent, Screen},
    storage::{Account, AccountSummary, Snapshot, Store},
    ui::{
        CaptureLayout, MainView, application, open_capture_window,
        workspace::{MusicTarget, Page},
    },
};

const BODY: &str = r#"{"result":{"id":924,"title":"Альбом из предзагрузки"}}"#;

#[test]
#[ignore = "renders diagnostics before and after a controlled native catalog opening"]
fn network_prefetch_metrics_follow_the_accepted_snapshot() {
    prove_network_prefetch(Trigger::Pin);
}

#[test]
#[ignore = "uses a rendered Home shelf and a loopback catalog in a fresh profile"]
fn visible_home_card_is_prefetched_while_clipped_shelves_wait() {
    prove_network_prefetch(Trigger::Viewport);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Trigger {
    Pin,
    Viewport,
}

fn prove_network_prefetch(trigger: Trigger) {
    assert!(env::var_os("MEDIADEKA_PROFILE_DIR").is_some());
    assert!(
        !Store::profile_directory()
            .unwrap()
            .join("mediadeka.sqlite3")
            .exists()
    );
    let output = PathBuf::from(env::var_os("MEDIADEKA_PREFETCH_METRICS_EVIDENCE").unwrap());
    fs::create_dir_all(&output).unwrap();
    let profile = Profile {
        id: "74369".into(),
        name: "Catalogue fixture".into(),
    };
    let provider = ProviderId::YandexMusic;
    let store = Store::open_default().unwrap();
    store
        .connect_account(
            &Session {
                provider,
                profile: profile.clone(),
                credentials: Credentials::YandexMusic {
                    access_token: "fixture-prefetch-metrics".into(),
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
    struct Disconnect(Store, String);
    impl Drop for Disconnect {
        fn drop(&mut self) {
            let _ = self
                .0
                .disconnect_account(ProviderId::YandexMusic, &self.1, || Some(()));
        }
    }
    let _disconnect = Disconnect(store.clone(), profile.id.clone());
    let account = store.account(provider, &profile.id).unwrap().unwrap();
    assert!(store.use_guest(|| Some(())).unwrap());
    let object = ObjectRef::native(provider, ObjectKind::Album, "924".into()).unwrap();
    let timestamp = now();
    assert!(
        store
            .claim_music_match(&region(), &object, timestamp, timestamp + 3_600)
            .unwrap()
    );
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    test_server::use_loopback(listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            match listener.accept() {
                Ok((stream, _)) => break respond(stream).unwrap(),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "catalog prefetch sent no request"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("{error}"),
            }
        }
    });
    let target = MusicTarget {
        object,
        profile: Some(profile.id),
        title: String::new(),
        artwork: None,
    };

    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let result = exercise(&output, target, store, account, trigger, cx).await;
            cx.update(|cx| cx.quit());
            result.unwrap();
        })
        .detach();
    });
    server.join().unwrap();
}

fn respond(mut stream: TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(4)))?;
    let mut request = BufReader::new(&stream);
    let mut line = String::new();
    request.read_line(&mut line)?;
    assert!(line.starts_with("GET /albums/924/with-tracks HTTP/"));
    loop {
        line.clear();
        if request.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
    }
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{BODY}",
        BODY.len(),
    )
}

async fn exercise(
    output: &Path,
    target: MusicTarget,
    store: Store,
    account: Account,
    trigger: Trigger,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let main = cx.update(|cx| {
        open_capture_window(
            cx,
            CaptureLayout {
                width: 1500.,
                height: 900.,
                reduced_motion: true,
            },
        )
    })?;
    let diagnostics = main.update(cx, |view, window, cx| {
        if trigger == Trigger::Viewport {
            show_viewport_home(view, &account, window, cx);
        } else {
            view.show_combined_home_for_capture(cx);
            view.prefetch_pins([target.clone()], cx);
        }
        view.open_sound_diagnostics_for_capture(window, cx)
    })??;
    enable_accessibility(diagnostics, cx)?;
    if trigger == Trigger::Viewport {
        let raw = AnyWindowHandle::from(main);
        enable_accessibility(raw, cx)?;
        raw.update(cx, |_, window, cx| -> gpui::Result<()> {
            window.draw(cx).clear(cx);
            window
                .render_to_image()?
                .save(output.join("home-viewport.png"))?;
            Ok(())
        })??;
    }
    wait_report(main, 0, BODY.len() as u64, cx).await?;
    if trigger == Trigger::Viewport {
        assert!(main.update(cx, |view, _, _| {
            view.music
                .prefetch
                .viewport
                .visible()
                .iter()
                .map(|key| key.object.id.as_str())
                .eq(["924"])
        })?);
        println!("Visible Home album 924 prefetched. The clipped shelf 925 had no catalog task.");
    }
    capture(
        diagnostics,
        output,
        "unused",
        &format!(
            "использовано 0 из 1 · ответы без использованного снимка {} Б",
            BODY.len()
        ),
        cx,
    )
    .await?;
    main.update(cx, |view, window, cx| {
        view.open_music_target(target, window, cx);
        assert!(view.music.prefetch.viewport.visible().is_empty());
    })?;
    wait_report(main, 1, 0, cx).await?;
    capture(
        diagnostics,
        output,
        "consumed",
        "использовано 1 из 1 · ответы без использованного снимка 0 Б",
        cx,
    )
    .await?;
    capture(
        diagnostics,
        output,
        "before-revocation",
        "Сбросы каталога · очистка 0 · аккаунт 0 · отзыв 0 · последний —",
        cx,
    )
    .await?;
    let revoked_store = store.clone();
    let (first, repeated) = cx
        .background_executor()
        .spawn(async move {
            Ok::<_, crate::storage::StoreError>((
                revoked_store.forget_music_account(&account)?,
                revoked_store.forget_music_account(&account)?,
            ))
        })
        .await
        .map_err(io::Error::other)?;
    assert_eq!((first, repeated), (true, false));
    capture(
        diagnostics,
        output,
        "revoked",
        "Сбросы каталога · очистка 0 · аккаунт 0 · отзыв 1 · последний отзыв доступа",
        cx,
    )
    .await?;

    let accounts = store.accounts().map_err(io::Error::other)?;
    main.update(cx, |view, window, cx| {
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
        view.show_settings_for_capture(window, cx);
        view.run(Intent::Disconnect(ProviderId::YandexMusic), cx);
    })?;
    capture(
        diagnostics,
        output,
        "disconnected",
        "Сбросы каталога · очистка 0 · аккаунт 1 · отзыв 1 · последний изменение аккаунта",
        cx,
    )
    .await?;
    Ok(())
}

fn show_viewport_home(
    view: &mut MainView,
    account: &Account,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<MainView>,
) {
    let items = (0..9)
        .map(|index| HomeItem {
            id: format!("album:{}", if index == 8 { 925 } else { 924 }),
            title: if index == 8 {
                "Альбом вне окна"
            } else {
                "Альбом из предзагрузки"
            }
            .into(),
            subtitle: String::new(),
            artwork: None,
            url: None,
        })
        .collect();
    let sections = (0..9)
        .map(|index| HomeSection {
            title: format!("Подборка {}", index + 1),
            start: index,
            len: 1,
            ..Default::default()
        })
        .collect();
    let request = view.state.begin(Intent::Startup);
    assert!(view.state.finish(
        &request,
        Screen::Home(HomeView {
            session: ActiveSession::Account {
                provider: account.provider,
                profile: account.profile.clone(),
                capabilities: Capabilities {
                    home: true,
                    profile_selection: false,
                    device_code: true
                },
            },
            content: HomeContent::Fresh(Snapshot::received(Home {
                title: "Главная".into(),
                items,
                sections
            })),
            accounts: vec![AccountSummary {
                provider: account.provider,
                profile: account.profile.clone()
            }],
            switch_issue: None,
        })
    ));
    view.show_music_page(Page::Home, window, cx);
}

async fn wait_report(
    main: WindowHandle<MainView>,
    useful: u64,
    unused: u64,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let report = main.update(cx, |view, _, _| view.prefetch_report())?;
        if (
            report.network_completed,
            report.useful,
            report.unused_response_bytes,
        ) == (1, useful, unused)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "Unexpected prefetch totals: ({}, {}, {})",
                report.network_completed, report.useful, report.unused_response_bytes
            ))
            .into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
}

fn enable_accessibility(window: AnyWindowHandle, cx: &mut AsyncApp) -> gpui::Result<()> {
    window.update(cx, |_, window, _| -> gpui::Result<()> {
        let handle = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(io::Error::other("Expected Win32").into());
        };
        const UIA_ROOT_OBJECT_ID: isize = -25;
        unsafe { SendMessageW(handle.hwnd.get() as _, WM_GETOBJECT, 0, UIA_ROOT_OBJECT_ID) };
        Ok(())
    })?
}

async fn capture(
    window: AnyWindowHandle,
    output: &Path,
    name: &str,
    expected: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let rendered = window.update(cx, |_, window, cx| -> gpui::Result<bool> {
            window.draw(cx).clear(cx);
            let viewport = window.viewport_size();
            window.dispatch_event(
                ScrollWheelEvent {
                    position: point(viewport.width / 2., viewport.height / 2.),
                    delta: ScrollDelta::Pixels(point(px(0.), px(-360.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.draw(cx).clear(cx);
            let tree = window.debug_a11y_tree_json().unwrap_or_default();
            if !tree.contains(expected) {
                return Ok(false);
            }
            fs::write(output.join(format!("{name}-a11y.json")), tree)?;
            window
                .render_to_image()?
                .save(output.join(format!("{name}.png")))?;
            Ok(true)
        })??;
        if rendered {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!("Diagnostics did not render: {expected}")).into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(40))
            .await;
    }
}
