use std::{
    env, fs,
    io::{self, BufRead, BufReader, Cursor, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp};

use crate::{
    providers::Home,
    state::{ActiveSession, HomeContent, HomeView, Intent, Screen},
    storage::{Snapshot, Store},
    telemetry,
    ui::{CaptureLayout, application, open_capture_window},
};

use super::super::{
    Artwork, ArtworkEdge, ArtworkRequest, ArtworkSource, cache, colors, decode, disk, http,
};

const URL: &str = "https://i.scdn.co/image/revalidation-native-fixture";

#[test]
#[ignore = "requires a native D3D11 window and isolated artwork profile"]
fn revalidated_pixels_and_palette_replace_the_current_native_cover() {
    assert!(env::var_os("MEDIADEKA_PROFILE_DIR").is_some());
    let profile = Store::profile_directory().unwrap();
    assert!(!profile.join("mediadeka.sqlite3").exists());
    let evidence = PathBuf::from(env::var_os("MEDIADEKA_ARTWORK_REVALIDATION_EVIDENCE").unwrap());
    fs::create_dir_all(&evidence).unwrap();
    let warm = png([181, 62, 31, 255]);
    let cool = png([23, 61, 192, 255]);
    let expected_bytes = [warm.len() as u64, cool.len() as u64];
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    http::loopback_for_capture(URL, listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        for (status, tag, body) in [
            ("200 OK", "\"warm\"", warm.as_slice()),
            ("304 Not Modified", "\"warm\"", &[][..]),
            ("200 OK", "\"cool\"", cool.as_slice()),
        ] {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "artwork did not revalidate");
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(4)))
                .unwrap();
            let mut request = BufReader::new(&stream);
            let mut conditional = false;
            loop {
                let mut line = String::new();
                if request.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                    break;
                }
                if line.to_ascii_lowercase().starts_with("if-none-match:") {
                    assert_eq!(line.trim().split_once(':').unwrap().1.trim(), "\"warm\"");
                    conditional = true;
                }
            }
            assert_eq!(conditional, status != "200 OK" || tag == "\"cool\"");
            write!(stream, "HTTP/1.1 {status}\r\nContent-Type: image/png\r\nETag: {tag}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            stream.write_all(body).unwrap();
        }
    });
    let outcome = Arc::new(Mutex::new(None));
    let result = Arc::clone(&outcome);
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let run = capture(cx, &profile, &evidence, expected_bytes).await;
            *result.lock().unwrap() = Some(run);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    outcome.lock().unwrap().take().unwrap().unwrap();
    server.join().unwrap();
}

async fn capture(
    cx: &mut AsyncApp,
    profile: &Path,
    out: &Path,
    expected: [u64; 2],
) -> gpui::Result<()> {
    let store = Store::open(profile).map_err(io::Error::other)?;
    let path = disk::path(profile, URL, None).expect("managed artwork URL");
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
    let baseline_bytes = telemetry::received().artwork_bytes;
    window.update(cx, |view, _, cx| {
        let startup = view.state.begin(Intent::Startup);
        view.state.finish(
            &startup,
            Screen::Home(HomeView {
                session: ActiveSession::Guest,
                content: HomeContent::Fresh(Snapshot::received(Home {
                    title: String::new(),
                    sections: Vec::new(),
                    items: Vec::new(),
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
    let before = wait(cx, |cx| {
        raw.update(cx, |_, window, cx| {
            window.draw(cx).clear(cx);
            cx.fetch_asset::<Artwork>(&request).flatten()
        })
    })
    .await?;
    let first = disk::read_entry(&path).unwrap().receipt;
    assert_eq!(first.content, before.content);
    assert_eq!(
        telemetry::received().artwork_bytes - baseline_bytes,
        expected[0]
    );
    frame(raw, &out.join("initial.png"), cx)?;
    assert!(
        raw.update(cx, |_, _, cx| cache::request_revalidation_for_capture(
            &request, cx
        ))?
    );
    let retained = wait(cx, |_| {
        Ok(disk::read_entry(&path)
            .map(|entry| entry.receipt)
            .filter(|receipt| *receipt != first))
    })
    .await?;
    assert_eq!(retained.content, first.content);
    assert_eq!(
        telemetry::received().artwork_bytes - baseline_bytes,
        expected[0]
    );
    let original_variant = raw.update(cx, |_, _, cx| {
        cx.fetch_asset::<Artwork>(&request)
            .flatten()
            .unwrap()
            .image
            .id
    })?;
    assert_eq!(original_variant, before.image.id);
    frame(raw, &out.join("not-modified.png"), cx)?;
    wait(cx, |cx| {
        raw.update(cx, |_, _, cx| {
            cache::request_revalidation_for_capture(&request, cx).then_some(())
        })
    })
    .await?;
    let after = wait(cx, |cx| {
        raw.update(cx, |_, window, cx| {
            window.draw(cx).clear(cx);
            cx.fetch_asset::<Artwork>(&request)
                .flatten()
                .filter(|image| image.content != before.content)
        })
    })
    .await?;
    assert_ne!(after.colors, before.colors);
    assert_eq!(
        telemetry::received().artwork_bytes - baseline_bytes,
        expected.into_iter().sum::<u64>()
    );
    let generation = disk::generation(&path).unwrap();
    for content in [before.content, after.content] {
        assert!(
            store
                .artwork_palette(&content, colors::ALGORITHM_VERSION, generation)
                .unwrap()
                .is_some()
        );
    }
    frame(raw, &out.join("changed.png"), cx)?;
    fs::write(
        out.join("report.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "response_body_bytes": [expected[0], 0, expected[1]],
            "same_render_image_on_304": true,
            "palette_before": before.colors.unwrap().swatches(),
            "palette_after": after.colors.unwrap().swatches(),
        }))?,
    )?;
    Ok(())
}

async fn wait<T>(
    cx: &mut AsyncApp,
    mut poll: impl FnMut(&mut AsyncApp) -> gpui::Result<Option<T>>,
) -> gpui::Result<T> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(value) = poll(cx)? {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Artwork validation state did not settle").into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
}

fn frame(raw: AnyWindowHandle, path: &Path, cx: &mut AsyncApp) -> gpui::Result<()> {
    raw.update(cx, |_, window, cx| -> gpui::Result<()> {
        window.draw(cx).clear(cx);
        window.render_to_image()?.save(path)?;
        Ok(())
    })?
}

fn png(pixel: [u8; 4]) -> Vec<u8> {
    let mut result = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(160, 160, image::Rgba(pixel)))
        .write_to(&mut result, image::ImageFormat::Png)
        .unwrap();
    result.into_inner()
}
