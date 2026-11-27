use std::{
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETOBJECT};

use crate::{
    music::{MusicPage, ObjectKind, ObjectRef, region},
    providers::HomeItem,
    telemetry::{self, CatalogInvalidation},
    ui::{CaptureLayout, application, open_capture_window, workspace::MusicTarget},
};

use super::*;

#[test]
#[ignore = "renders Settings through GPUI/D3D11 in a fresh isolated profile"]
fn native_settings_catalog_cache_clear() {
    let evidence = PathBuf::from(env::var_os("MEDIADEKA_CATALOG_CLEAR_EVIDENCE").unwrap());
    assert!(env::var_os("MEDIADEKA_PROFILE_DIR").is_some());
    let profile = Store::profile_directory().unwrap();
    assert!(!profile.join("mediadeka.sqlite3").exists());
    let store = Store::open_default().unwrap();
    assert!(
        store
            .save_music_snapshot(ProviderId::YandexMusic, None, "ZZ", "album:9", &"old")
            .unwrap()
    );
    let artist = reference(ObjectKind::Artist, "400");
    let single = reference(ObjectKind::Album, "401");
    let mut catalogue = MusicPage::new(artist.clone(), "Артист для проверки".into());
    catalogue.releases = [
        (&single, "Сингл для проверки"),
        (&reference(ObjectKind::Album, "402"), "Другой релиз"),
    ]
    .into_iter()
    .map(|(release, title)| HomeItem {
        id: format!("album:{}", release.id),
        title: title.to_owned(),
        subtitle: String::new(),
        url: None,
        artwork: None,
    })
    .collect();
    let mut release = MusicPage::new(single.clone(), "Сингл для проверки".into());
    release.single = true;
    save_page(&store, &catalogue);
    save_page(&store, &release);
    // These local snapshots already carry the complete metadata for this regression.
    for reference in [&artist, &single] {
        store
            .claim_music_match(&region(), reference, 0, u64::MAX)
            .unwrap();
    }

    let result = Arc::new(Mutex::new(None));
    let completion = result.clone();
    let profile_store = store.clone();
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let captured: gpui::Result<()> = async {
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
                let raw = AnyWindowHandle::from(window);
                window.update(cx, |view, _, cx| {
                    view.show_combined_home_for_capture(cx);
                })?;
                raw.update(cx, |_, window, cx| -> gpui::Result<()> {
                    let handle = window.window_handle().map_err(io::Error::other)?;
                    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
                        return Err(io::Error::other("Expected Win32").into());
                    };
                    const UIA_ROOT_OBJECT_ID: isize = -25;
                    unsafe {
                        SendMessageW(handle.hwnd.get() as _, WM_GETOBJECT, 0, UIA_ROOT_OBJECT_ID)
                    };
                    window.draw(cx).clear(cx);
                    Ok(())
                })??;
                std::fs::create_dir_all(&evidence)?;
                open_page(window, artist, cx).await?;
                open_page(window, single.clone(), cx).await?;
                capture_discography(raw, &evidence, "single-before-catalog-clear", true, cx)?;
                window.update(cx, |view, window, cx| {
                    view.show_settings_for_capture(window, cx)
                })?;
                let before = telemetry::catalog().invalidations;
                assert_eq!((before.cleared, before.latest), (0, None));
                window.update(cx, |view, _, cx| {
                    view.set_home_scroll_offset_for_capture(300., cx);
                    request(view, cx);
                })?;
                let deadline = Instant::now() + Duration::from_secs(6);
                while !window.update(cx, |_, _, cx| {
                    matches!(cx.global::<Operation>().phase, Phase::Cleared)
                })? {
                    if Instant::now() >= deadline {
                        return Err(io::Error::other("Catalog clear did not finish").into());
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(20))
                        .await;
                }
                let after = telemetry::catalog().invalidations;
                assert_eq!(
                    (after.cleared, after.latest),
                    (1, Some(CatalogInvalidation::CacheCleared))
                );
                std::fs::write(
                    evidence.join("catalog-invalidations.json"),
                    serde_json::to_vec_pretty(&serde_json::json!({
                        "explicit_clears_before": before.cleared,
                        "explicit_clears_after": after.cleared,
                        "latest_after": "cache_cleared",
                    }))?,
                )?;
                raw.update(cx, |_, window, cx| -> gpui::Result<()> {
                    window.draw(cx).clear(cx);
                    window
                        .render_to_image()?
                        .save(evidence.join("catalog-cleared.png"))?;
                    let tree = window
                        .debug_a11y_tree_json()
                        .ok_or_else(|| io::Error::other("Missing accessibility tree"))?;
                    std::fs::write(evidence.join("catalog-cleared-a11y.json"), &tree)?;
                    if !tree.contains("Очистить кэш обложек")
                        || !tree.contains("Очистить снимки страниц и плейлистов")
                        || !tree.contains("Снимки каталога очищены.")
                    {
                        return Err(io::Error::other("Catalog clear semantics missing").into());
                    }
                    Ok(())
                })??;
                // Reopen the single from a new snapshot without restoring the artist snapshot.
                save_page(&profile_store, &release);
                open_page(window, single, cx).await?;
                capture_discography(raw, &evidence, "single-after-catalog-clear", false, cx)?;
                Ok(())
            }
            .await;
            *completion.lock().unwrap() = Some(captured);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    result.lock().unwrap().take().unwrap().unwrap();
    assert!(
        store
            .music_snapshot::<String>(ProviderId::YandexMusic, None, "ZZ", "album:9")
            .unwrap()
            .is_none()
    );
}

fn reference(kind: ObjectKind, id: &str) -> ObjectRef {
    ObjectRef {
        provider: ProviderId::YandexMusic,
        kind,
        id: id.to_owned(),
    }
}

fn save_page(store: &Store, page: &MusicPage) {
    assert!(
        store
            .save_music_snapshot(
                page.reference.provider,
                None,
                &region(),
                &format!("{}:{}", page.reference.kind.path(), page.reference.id),
                page,
            )
            .unwrap()
    );
}

async fn open_page(
    window: WindowHandle<MainView>,
    object: ObjectRef,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let kind = object.kind.path();
    window.update(cx, |view, window, cx| {
        view.open_music_target(
            MusicTarget {
                object,
                profile: None,
                title: String::new(),
                artwork: None,
            },
            window,
            cx,
        );
    })?;
    let deadline = Instant::now() + Duration::from_secs(6);
    while !window.update(cx, |view, _, _| {
        view.music_semantics_for_capture()["details"]["kind"].as_str() == Some(kind)
    })? {
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!("Timed out opening seeded {kind}")).into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    Ok(())
}

fn capture_discography(
    handle: AnyWindowHandle,
    evidence: &std::path::Path,
    name: &str,
    expected: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    handle.update(cx, |_, window, cx| -> gpui::Result<()> {
        window.draw(cx).clear(cx);
        window
            .render_to_image()?
            .save(evidence.join(format!("{name}.png")))?;
        let tree = window
            .debug_a11y_tree_json()
            .ok_or_else(|| io::Error::other("Missing accessibility tree"))?;
        std::fs::write(evidence.join(format!("{name}-a11y.json")), &tree)?;
        if tree.contains("Другие релизы артиста") != expected {
            return Err(io::Error::other(format!("Unexpected discography at {name}")).into());
        }
        Ok(())
    })?
}
