use std::{
    env, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, Pixels, ScrollDelta, ScrollWheelEvent,
    WindowHandle, point, px,
};

use crate::ui::{CaptureLayout, MainView, application, open_capture_window, seed_playlist_capture};

#[test]
#[ignore = "requires Windows D3D11, an isolated profile, and a WAV fixture"]
fn native_playlist_scroll_effects() {
    let fixture = PathBuf::from(env::var_os("MEDIADEKA_EFFECTS_WAV").unwrap());
    let output = PathBuf::from(env::var_os("MEDIADEKA_EFFECTS_OUTPUT").unwrap());
    let profile = PathBuf::from(env::var_os("MEDIADEKA_PROFILE_DIR").unwrap());
    let playlist = seed_playlist_capture(&profile, &vec![fixture; 70]).unwrap();
    let result = Arc::new(Mutex::new(None));
    let destination = Arc::clone(&result);
    application().run(move |cx| {
        let window = open_capture_window(
            cx,
            CaptureLayout {
                width: 1100.,
                height: 700.,
                reduced_motion: true,
            },
        )
        .unwrap();
        cx.spawn(async move |cx| {
            let check = capture(window, playlist, &output, cx).await;
            *destination.lock().unwrap() = Some(check);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    result.lock().unwrap().take().unwrap().unwrap();
}

async fn capture(
    view: WindowHandle<MainView>,
    playlist: i64,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let executor = cx.background_executor().clone();
    executor.timer(Duration::from_millis(200)).await;
    view.update(cx, |view, window, cx| {
        view.open_local_playlist_for_capture(playlist, window, cx)
    })?;
    let start = Instant::now();
    loop {
        if view
            .update(cx, |view, _, _| view.playlist_status_for_capture())?
            .transpose()
            .map_err(io::Error::other)?
            == Some(70)
        {
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            return Err(io::Error::other("Не открылся плейлист проверки эффектов.").into());
        }
        executor.timer(Duration::from_millis(20)).await;
    }
    let raw = AnyWindowHandle::from(view);
    raw.update(cx, |_, window, cx| -> gpui::Result<()> {
        window.draw(cx).clear(cx);
        let before = window.render_to_image()?;
        before.save(output.join("playlist-before-scroll.png"))?;
        window.dispatch_event(
            ScrollWheelEvent {
                position: point(px(630.), px(295.)),
                delta: ScrollDelta::Pixels(point(Pixels::ZERO, -px(6.))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.draw(cx).clear(cx);
        let during = window.render_to_image()?;
        during.save(output.join("playlist-during-scroll.png"))?;
        assert_ne!(
            before.as_raw(),
            during.as_raw(),
            "Прокрутка должна изменить кадр"
        );
        Ok(())
    })?
}
