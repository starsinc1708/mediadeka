use std::{
    env, fs, io,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, InputEvent, KeyUpEvent, Keystroke, WindowHandle, point, px};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::Value;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowRect, SendMessageW, WM_DPICHANGED, WM_GETOBJECT,
};

use crate::{
    music::{MusicPage, ObjectKind, ObjectRef, region},
    playback::Status,
    providers::{ProviderId, catalog},
    storage::Store,
    ui::{
        CaptureLayout, MainView, application, open_capture_window,
        workspace::{MusicTarget, Route},
    },
};

const TEST: &str = "ui::music::detail::related_acceptance::live_artist_profile_navigation";
const CHILD: &str = "MEDIADEKA_NATIVE_RELATED_CHILD";
const EVIDENCE: &str = "MEDIADEKA_NATIVE_RELATED_EVIDENCE";
const AUDIO: &str = "MEDIADEKA_NATIVE_RELATED_AUDIO";
const ORIGIN: &str = "UCc4K7bAqpdBP8jh1j9XZAww";
const PROVIDER: ProviderId = ProviderId::YouTubeMusic;
const LEGACY_CURRENT: &str = "Cached current artist";
const LEGACY_DUPLICATE: &str = "Cached duplicate artist";

#[test]
#[ignore = "reads public artist profiles and opens their real cached pages with native GPUI and libmpv"]
fn live_artist_profile_navigation() {
    if env::var_os(CHILD).is_none() {
        let cancelled = AtomicBool::new(false);
        let origin = reference(ORIGIN);
        let mut page = catalog::read(&origin, None, &cancelled).unwrap();
        let related = &mut page.artist_details.as_mut().unwrap().related;
        let target = ObjectRef::from_item(PROVIDER, &related[0]).unwrap();
        let mut current = related[0].clone();
        current.id = format!("browse:{}", origin.id);
        current.title = LEGACY_CURRENT.into();
        let mut duplicate = related[0].clone();
        duplicate.title = LEGACY_DUPLICATE.into();
        related.insert(0, current);
        related.insert(2, duplicate);
        let next = catalog::read(&target, None, &cancelled).unwrap();
        let audio = PathBuf::from(env::var_os(AUDIO).expect("path to a test audio file"));
        assert!(audio.is_file());
        for mode in ["1920", "800-150"] {
            let profile = tempfile::tempdir().unwrap();
            let store = Store::open(profile.path()).unwrap();
            for data in [&page, &next] {
                let slot = format!("artist:{}", data.reference.id);
                assert!(
                    store
                        .save_music_snapshot(PROVIDER, None, &region(), &slot, data)
                        .unwrap()
                );
            }
            let local = store
                .create_local_playlist("Related artists acceptance", std::slice::from_ref(&audio))
                .unwrap();
            assert!(
                Command::new(env::current_exe().unwrap())
                    .args(["--exact", TEST, "--ignored", "--nocapture"])
                    .env(CHILD, mode)
                    .env("MEDIADEKA_PROFILE_DIR", profile.path())
                    .env("MEDIADEKA_GUI", "3")
                    .env("MEDIADEKA_NATIVE_RELATED_LOCAL", local.to_string())
                    .status()
                    .unwrap()
                    .success()
            );
        }
        return;
    }

    let mode = env::var(CHILD).unwrap();
    let origin = reference(ORIGIN);
    let store = Store::open_default().unwrap();
    let page: MusicPage = store
        .music_snapshot(PROVIDER, None, &region(), &format!("artist:{ORIGIN}"))
        .unwrap()
        .unwrap();
    let id = env::var("MEDIADEKA_NATIVE_RELATED_LOCAL")
        .unwrap()
        .parse()
        .unwrap();
    let result = Arc::new(Mutex::new(None));
    let completion = Arc::clone(&result);
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            *completion.lock().unwrap() = Some(verify(cx, &mode, id, &page, origin).await);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    result.lock().unwrap().take().unwrap().unwrap();
}

fn reference(id: &str) -> ObjectRef {
    ObjectRef::native(PROVIDER, ObjectKind::Artist, id.into()).unwrap()
}

async fn verify(
    cx: &mut AsyncApp,
    mode: &str,
    local: i64,
    page: &MusicPage,
    origin: ObjectRef,
) -> gpui::Result<()> {
    let compact = mode == "800-150";
    let window = cx.update(|cx| {
        open_capture_window(
            cx,
            CaptureLayout {
                width: if compact { 800. } else { 1_920. },
                height: if compact { 400. } else { 1_080. },
                reduced_motion: true,
            },
        )
    })?;
    enable_accessibility(window, compact, cx)?;
    window.update(cx, |view, window, cx| {
        view.open_local_playlist_for_capture(local, window, cx)
    })?;
    wait_for(window, cx, "local playlist", |view, _| {
        view.playlist_status_for_capture() == Some(Ok(1))
    })
    .await?;
    window.update(cx, |view, _, cx| {
        view.play_playlist_track_for_capture(0, cx)
    })?;
    wait_for(window, cx, "local playback", |view, cx| {
        matches!(view.player_snapshot.status, Status::Playing)
            && view.player.read(cx).position() > 0.05
    })
    .await?;
    let title = page.title.clone();
    window.update(cx, |view, window, cx| {
        view.open_music_target(
            MusicTarget {
                object: origin.clone(),
                profile: None,
                title: title.clone(),
                artwork: page.artwork.clone(),
            },
            window,
            cx,
        )
    })?;
    wait_for(window, cx, &title, |view, _| {
        view.music_semantics_for_capture()["details"]["title"] == title
    })
    .await?;
    let details = page.artist_details.as_ref().unwrap();
    let portrait = draw(window, mode, "artist", cx)?;
    assert!(portrait.contains(details.monthly_listeners_caption.as_deref().unwrap()));
    assert!(portrait.contains("music-object-artwork"));
    assert!(!portrait.contains(LEGACY_CURRENT) && !portrait.contains(LEGACY_DUPLICATE));
    let target = ObjectRef::from_item(PROVIDER, &details.related[1]).unwrap();
    let destination_title = &details.related[1].title;
    let before_count = window.update(cx, |view, _, _| view.player_snapshot.queue_len)?;
    let position = window.update(cx, |view, _, cx| view.player.read(cx).position())?;
    window.update(cx, |view, _, cx| {
        let offset = view
            .scroll
            .max_offset()
            .y
            .min(px(if compact { 270. } else { 130. }));
        view.scroll.set_offset(point(px(0.), -offset));
        cx.notify();
    })?;
    let tree = tab_to(window, destination_title, cx)?;
    if compact {
        assert_eq!(tree["frame"]["scale_factor"], 1.5);
    }
    let focused = draw(window, mode, "related-focused", cx)?;
    assert!(focused.contains("music-object-artwork"));
    if !compact {
        let cover = window.update(cx, |view, window, _| {
            view.rendered_hero_artwork_for_capture(window)
        })?;
        assert!(
            cover.is_some_and(|bounds| bounds.size.width > px(120.)),
            "the wide reduced-motion Hero must still occupy its remaining scroll footprint"
        );
    }
    let scroll = window.update(cx, |view, _, _| view.scroll.offset())?;
    assert!(scroll.y < px(0.));
    let enter = Keystroke::parse("enter")?;
    key(window, &enter, cx)?;
    wait_for(window, cx, destination_title, |view, _| {
        matches!(view.workspace.routes.current(), Route::Object(route) if route.object == target)
            && view.music_semantics_for_capture()["details"]["title"] == destination_title.as_str()
    })
    .await?;
    draw(window, mode, "related-profile", cx)?;
    window.update(cx, |view, window, cx| {
        view.playlist_history_for_capture(false, window, cx)
    })?;
    wait_for(window, cx, "return to artist", |view, _| {
        matches!(view.workspace.routes.current(), Route::Object(route) if route.object == origin)
            && view.music_semantics_for_capture()["details"]["title"] == title
    })
    .await?;
    draw(window, mode, "returned", cx)?;
    let resumed = window.update(cx, |view, _, cx| {
        assert!(matches!(view.player_snapshot.status, Status::Playing));
        assert_eq!(view.player_snapshot.queue_len, before_count);
        let resumed = view.player.read(cx).position();
        assert!(resumed > position);
        assert!((view.scroll.offset().y - scroll.y).abs() < px(2.));
        view.scroll
            .set_offset(point(px(0.), -view.scroll.max_offset().y));
        cx.notify();
        resumed
    })?;
    let end = draw(window, mode, "biography", cx)?;
    assert!(end.contains("Об исполнителе"));
    println!(
        "{mode}: {} → {} ({}) → Back, playback {position:.2} → {resumed:.2} s, scroll restored {:.1} DIP",
        title,
        destination_title,
        target.id,
        f32::from(scroll.y)
    );
    Ok(())
}

async fn wait_for(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    label: &str,
    ready: impl Fn(&MainView, &gpui::Context<MainView>) -> bool,
) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(6);
    while !window.update(cx, |view, _, cx| ready(view, cx))? {
        if Instant::now() > deadline {
            return Err(io::Error::other(format!("Timed out waiting for {label}")).into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    }
    Ok(())
}

fn key(
    window: WindowHandle<MainView>,
    keystroke: &Keystroke,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    AnyWindowHandle::from(window).update(cx, |_, window, cx| {
        window.dispatch_keystroke(keystroke.clone(), cx);
        window.dispatch_event(
            KeyUpEvent {
                keystroke: keystroke.clone(),
            }
            .to_platform_input(),
            cx,
        );
        window.draw(cx).clear(cx);
    })
}

fn tab_to(window: WindowHandle<MainView>, title: &str, cx: &mut AsyncApp) -> gpui::Result<Value> {
    let tab = Keystroke::parse("tab")?;
    let mut tree: Value = serde_json::from_str(&draw(window, "", "", cx)?)?;
    let stops = tree["frame"]["tab_stop_count"].as_u64().unwrap_or(64);
    for _ in 0..=stops {
        let focus = tree["gpui_focus"].as_str().unwrap_or_default();
        let node = &tree["nodes"][focus];
        if node["aria"]["label"] == title
            && node["element_id"]
                .as_str()
                .is_some_and(|id| id.contains("music-related-artist"))
        {
            return Ok(tree);
        }
        key(window, &tab, cx)?;
        tree = serde_json::from_str(&draw(window, "", "", cx)?)?;
    }
    Err(io::Error::other(format!("Tab did not focus related artist {title}")).into())
}

fn draw(
    window: WindowHandle<MainView>,
    mode: &str,
    name: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<String> {
    AnyWindowHandle::from(window).update(cx, |_, window, cx| {
        window.draw(cx).clear(cx);
        let tree = window
            .debug_a11y_tree_json()
            .ok_or_else(|| io::Error::other("Missing accessibility tree"))?;
        if !name.is_empty()
            && let Some(path) = env::var_os(EVIDENCE)
        {
            let directory = PathBuf::from(path).join(mode);
            fs::create_dir_all(&directory)?;
            window
                .render_to_image()?
                .save(directory.join(format!("{name}.png")))?;
            fs::write(directory.join(format!("{name}-a11y.json")), &tree)?;
        }
        Ok(tree)
    })?
}

fn enable_accessibility(
    window: WindowHandle<MainView>,
    scaled: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let (hwnd, rect) = window.update(cx, |_, window, _| -> gpui::Result<_> {
        let raw = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::Win32(handle) = raw.as_raw() else {
            return Err(io::Error::other("Expected a native Win32 window").into());
        };
        let hwnd = handle.hwnd.get() as _;
        let (mut rect, mut client) = (Default::default(), Default::default());
        // Win32 writes into stack rectangles while the borrowed native window is live.
        if unsafe { GetWindowRect(hwnd, &mut rect) == 0 || GetClientRect(hwnd, &mut client) == 0 } {
            return Err(io::Error::last_os_error().into());
        }
        rect.right += 1_200 - (client.right - client.left);
        rect.bottom += 600 - (client.bottom - client.top);
        Ok((hwnd, rect))
    })??;
    // The messages run synchronously with no borrow of the GPUI view.
    unsafe {
        if scaled {
            SendMessageW(
                hwnd,
                WM_DPICHANGED,
                144 | 144 << 16,
                &rect as *const _ as isize,
            );
        }
        SendMessageW(hwnd, WM_GETOBJECT, 0, -25);
    }
    Ok(())
}
