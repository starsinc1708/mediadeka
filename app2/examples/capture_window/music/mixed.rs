use std::{
    env, fs, io,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::MainView;
use serde_json::json;

use super::{ALBUM, Scene, draw};

const ROUTES: [(&str, &str, &str); 2] = [
    ("yandex-album", ALBUM.1, "album"),
    (
        "youtube-track",
        "https://music.youtube.com/watch?v=4D7u5KF7SP8",
        "track",
    ),
];

pub(super) async fn verify(
    view: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(view);
    let duration = env::var("MEDIADEKA_ENTITY_SOAK_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(Duration::ZERO, Duration::from_secs);
    let mut opens = Vec::new();
    let timer = cx.background_executor().clone();
    let begin = Instant::now();
    loop {
        for (name, address, kind) in ROUTES {
            let started = Instant::now();
            view.update(cx, |view, window, cx| {
                view.submit_music_url_for_capture(address, window, cx)
            })?;
            let deadline = started + Duration::from_secs(20);
            let (title, elapsed) = loop {
                raw.update(cx, |_, window, cx| {
                    window.simulate_next_frame(cx);
                    window.draw(cx).clear(cx);
                })?;
                let (model, elapsed) = view.update(cx, |view, _, _| {
                    (
                        view.music_semantics_for_capture(),
                        view.route_content_ms_for_capture(),
                    )
                })?;
                if model["details"]["kind"] == kind
                    && model["details"]["tracks"]
                        .as_u64()
                        .is_some_and(|count| count > 0)
                    && let Some(title) = model["details"]["title"].as_str()
                    && let Some(elapsed) = elapsed
                {
                    break (title.to_owned(), elapsed);
                }
                if Instant::now() >= deadline
                    || (model["pending"] == false && model["details"]["error"].is_string())
                {
                    return Err(io::Error::other(format!("{name}: {model}")).into());
                }
                timer.timer(Duration::from_millis(25)).await;
            };
            if opens.len() < ROUTES.len() {
                timer.timer(Duration::from_millis(350)).await;
                draw(raw, scene, Some(&output.join(format!("{name}.png"))), cx)?;
                println!("{name} · {title} · {elapsed:.2} мс");
            }
            opens.push(json!({"route": name, "title": title, "elapsed_ms": elapsed}));
        }
        if begin.elapsed() >= duration {
            break;
        }
        timer.timer(Duration::from_millis(120)).await;
    }
    fs::write(
        output.join("mixed-catalog.json"),
        serde_json::to_vec_pretty(&json!({
            "duration_ms": begin.elapsed().as_millis(), "opens": opens
        }))?,
    )?;
    Ok(())
}
