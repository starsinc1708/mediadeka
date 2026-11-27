use std::{
    env, fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp};

use crate::{
    providers::{Home, HomeItem, HomeSection},
    state::{ActiveSession, HomeContent, HomeView, Intent, Screen},
    storage::Snapshot,
    ui::{CaptureLayout, application, open_capture_window, workspace::Page},
};

use super::{ArtworkSource, loaded_for_capture};

#[test]
#[ignore = "renders a native artist Hero from the same real image across isolated profile restarts"]
fn artist_palette_reuses_the_profile_database_after_restart() {
    use std::sync::atomic::Ordering;

    use super::super::{
        Artwork, ArtworkClearStatus, ArtworkEdge, ArtworkRequest, artwork_clear_status,
        clear_artwork_cache, colors, decode, disk, palette, seed_cover_for_capture,
    };
    use crate::storage::Store;

    const URL: &str = "https://i.scdn.co/image/palette-restart-fixture";
    assert!(env::var_os("MEDIADEKA_PROFILE_DIR").is_some());
    let phase = env::var("MEDIADEKA_ARTWORK_PALETTE_PHASE").unwrap();
    let output = PathBuf::from(env::var_os("MEDIADEKA_ARTWORK_FRAME").unwrap());
    let profile = Store::profile_directory().unwrap();
    let store = Store::open(&profile).unwrap();
    if phase == "cold" {
        let fixture = PathBuf::from(env::var_os("MEDIADEKA_ARTWORK_PALETTE_FIXTURE").unwrap());
        seed_cover_for_capture(&profile, URL, &fixture).unwrap();
    }
    let path = disk::path(&profile, URL, None).unwrap();
    let content = disk::read_entry(&path).unwrap().receipt.content;
    let generation = disk::generation(&path).unwrap();
    let cold = phase == "cold";
    assert_eq!(
        store
            .artwork_palette(&content, colors::ALGORITHM_VERSION, generation)
            .unwrap()
            .is_none(),
        cold
    );

    let result = Arc::new(Mutex::new(None));
    let complete = Arc::clone(&result);
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let captured: gpui::Result<()> = async {
                let window = cx.update(|cx| {
                    open_capture_window(
                        cx,
                        CaptureLayout {
                            width: 1500.,
                            height: 800.,
                            reduced_motion: true,
                        },
                    )
                })?;
                window.update(cx, |view, _, cx| {
                    let request = view.state.begin(Intent::Startup);
                    view.state.finish(
                        &request,
                        Screen::Home(HomeView {
                            session: ActiveSession::Guest,
                            content: HomeContent::Fresh(Snapshot::received(Home {
                                title: String::new(),
                                items: Vec::new(),
                                sections: Vec::new(),
                            })),
                            accounts: Vec::new(),
                            switch_issue: None,
                        }),
                    );
                    view.show_artist_artwork_for_capture(Some(URL), None, cx);
                })?;
                let raw = AnyWindowHandle::from(window);
                let request = ArtworkRequest {
                    source: ArtworkSource::new(URL, false),
                    edge: ArtworkEdge::new(colors::SAMPLE_EDGE as f32, 1.),
                    treatment: decode::Treatment::Original,
                };
                let started = Instant::now();
                let swatches = loop {
                    let available = raw.update(cx, |_, window, cx| {
                        window.draw(cx).clear(cx);
                        cx.fetch_asset::<Artwork>(&request)
                            .flatten()
                            .and_then(|artwork| artwork.colors)
                    })?;
                    if let Some(colors) = available {
                        break colors;
                    }
                    if started.elapsed() >= Duration::from_secs(8) {
                        return Err(io::Error::other(
                            "Persisted artist palette did not reach the native window",
                        )
                        .into());
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(15))
                        .await;
                };
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.;
                raw.update(cx, |_, window, cx| -> gpui::Result<()> {
                    window.draw(cx).clear(cx);
                    window.render_to_image()?.save(&output)?;
                    Ok(())
                })??;
                let extractions = palette::EXTRACTIONS.load(Ordering::Relaxed);
                assert_eq!(extractions, u64::from(cold));
                let mut cleared = false;
                if phase == "clear" {
                    window.update(cx, |view, window, cx| {
                        view.show_settings_for_capture(window, cx);
                        clear_artwork_cache(cx);
                    })?;
                    let deadline = Instant::now() + Duration::from_secs(8);
                    while !raw.update(cx, |_, _, cx| {
                        artwork_clear_status(cx) == ArtworkClearStatus::Cleared
                    })? {
                        if Instant::now() >= deadline {
                            return Err(io::Error::other("Palette clear did not finish").into());
                        }
                        cx.background_executor()
                            .timer(Duration::from_millis(15))
                            .await;
                    }
                    cleared = true;
                }
                fs::write(
                    output.with_extension("json"),
                    serde_json::to_vec_pretty(&serde_json::json!({
                        "phase": phase,
                        "palette_ms": elapsed_ms,
                        "extractions": extractions,
                        "swatches": swatches.swatches(),
                        "cleared": cleared,
                    }))?,
                )?;
                Ok(())
            }
            .await;
            *complete.lock().unwrap() = Some(captured);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    result.lock().unwrap().take().unwrap().unwrap();
    let cleared = !path.is_file();
    assert_eq!(
        store
            .artwork_palette(&content, colors::ALGORITHM_VERSION, generation)
            .unwrap()
            .is_none(),
        cleared
    );
}

#[test]
#[ignore = "requires a capture profile, public CDN fixtures and a native Windows D3D11 window"]
fn home_cards_render_with_public_cdn_artwork() {
    let fixtures = PathBuf::from(env::var_os("MEDIADEKA_ARTWORK_FIXTURES").unwrap());
    let frame = PathBuf::from(env::var_os("MEDIADEKA_ARTWORK_FRAME").unwrap());
    assert!(env::var_os("MEDIADEKA_PROFILE_DIR").is_some());
    let samples: Vec<(String, String)> =
        serde_json::from_slice(&fs::read(fixtures).unwrap()).unwrap();
    assert_eq!(samples.len(), 4);
    let outcome = Arc::new(Mutex::new(None));
    let result = Arc::clone(&outcome);
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let check = capture(cx, samples, &frame).await;
            *result.lock().unwrap() = Some(check);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    outcome.lock().unwrap().take().unwrap().unwrap();
}

async fn capture(
    cx: &mut AsyncApp,
    samples: Vec<(String, String)>,
    frame: &Path,
) -> gpui::Result<()> {
    let window = cx.update(|cx| {
        open_capture_window(
            cx,
            CaptureLayout {
                width: 1500.,
                height: 800.,
                reduced_motion: true,
            },
        )
    })?;
    let requests = samples
        .iter()
        .map(|(_, url)| ArtworkSource::new(url, true))
        .collect::<Vec<_>>();
    let title = "Обложки главной".to_owned();
    let home = Home {
        sections: vec![HomeSection {
            title: title.clone(),
            len: samples.len(),
            ..Default::default()
        }],
        title,
        items: samples
            .into_iter()
            .enumerate()
            .map(|(index, (label, url))| HomeItem {
                id: index.to_string(),
                title: label,
                subtitle: "Проверка обложки".into(),
                url: None,
                artwork: Some(url),
            })
            .collect(),
    };
    cx.background_executor()
        .timer(Duration::from_millis(80))
        .await;
    let start = Instant::now();
    window.update(cx, |view, window, cx| {
        let startup = view.state.begin(Intent::Startup);
        view.state.finish(
            &startup,
            Screen::Home(HomeView {
                session: ActiveSession::Guest,
                content: HomeContent::Fresh(Snapshot::received(home)),
                accounts: Vec::new(),
                switch_issue: None,
            }),
        );
        view.show_music_page(Page::Home, window, cx);
        cx.notify();
    })?;

    let raw = AnyWindowHandle::from(window);
    raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
    // Fluid Glass can place the provider shelf below the introductory sections.
    window.update(cx, |view, _, cx| {
        let shelf = view.home_shelf_origin_for_capture();
        view.set_home_scroll_offset_for_capture((shelf - 12.).max(0.), cx);
    })?;
    raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
    let save_frame = |cx: &mut AsyncApp| {
        raw.update(cx, |_, window, cx| -> gpui::Result<()> {
            window.draw(cx).clear(cx);
            window.render_to_image()?.save(frame)?;
            Ok(())
        })?
    };
    loop {
        let ready = raw.update(cx, |_, _, cx| -> gpui::Result<bool> {
            let mut ready = true;
            for (index, request) in requests.iter().enumerate() {
                match loaded_for_capture(request, cx) {
                    Some(true) => {}
                    None => ready = false,
                    Some(false) => {
                        return Err(io::Error::other(format!(
                            "Cover {} failed to load",
                            index + 1
                        ))
                        .into());
                    }
                }
            }
            Ok(ready)
        })??;
        if ready {
            break;
        }
        if start.elapsed() >= Duration::from_secs(18) {
            save_frame(cx)?;
            return Err(io::Error::other("Four visible covers did not decode").into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(15))
            .await;
        raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
    }
    println!(
        "Four covers ready for the window after {} ms",
        start.elapsed().as_millis()
    );
    save_frame(cx)
}
