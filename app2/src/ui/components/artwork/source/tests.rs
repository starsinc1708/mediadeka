use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Cursor, Write},
    net::TcpListener,
    thread,
    time::{Duration, SystemTime},
};

use crate::storage::Store;

use super::*;
use crate::ui::components::artwork::{colors, palette};

const URL: &str = "https://i.scdn.co/image/managed-palette";

#[test]
fn real_http_revalidation_keeps_304_pixels_offline_and_refreshes_a_changed_image() {
    let profile = tempfile::tempdir().unwrap();
    let store = Store::open(profile.path()).unwrap();
    let source = ArtworkSource::new(URL, false);
    let path = disk::path(profile.path(), URL, None).unwrap();
    let generation = disk::generation(&path).unwrap();
    let cancel = AtomicBool::new(false);
    let first = png([181, 60, 28, 255]);
    let changed = png([21, 52, 177, 255]);
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let local = format!("http://{}/cover.png", listener.local_addr().unwrap());
    let old_size = first.len();
    let new_size = changed.len();
    let server = thread::spawn(move || {
        for (condition, status, headers, body) in [
            (
                None,
                "200 OK",
                "ETag: \"first\"\r\nLast-Modified: Fri, 18 Oct 2024 02:19:00 GMT\r\n",
                first.as_slice(),
            ),
            (
                Some("if-none-match: \"first\""),
                "304 Not Modified",
                "",
                &[][..],
            ),
            (
                Some("if-none-match: \"first\""),
                "200 OK",
                "Last-Modified: Sat, 19 Oct 2024 02:19:00 GMT\r\n",
                changed.as_slice(),
            ),
            (
                Some("if-modified-since: sat, 19 oct 2024 02:19:00 gmt"),
                "304 Not Modified",
                "",
                &[][..],
            ),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = BufReader::new(&stream);
            let mut condition_matched = false;
            loop {
                let mut line = String::new();
                if request.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                    break;
                }
                let header = line.trim().to_ascii_lowercase();
                if header.starts_with("if-") {
                    assert_eq!(Some(header.as_str()), condition);
                    condition_matched = true;
                }
            }
            assert_eq!(condition_matched, condition.is_some());
            write!(stream, "HTTP/1.1 {status}\r\nContent-Type: image/png\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            stream.write_all(body).unwrap();
        }
    });
    let fetch = |_: &str, validators: Option<&Validators>| {
        http::request(&ureq::Agent::new_with_defaults(), &local, validators)
    };
    let Some(Response::Image {
        image,
        validators,
        no_store: false,
    }) = fetch(URL, None)
    else {
        panic!("initial response failed");
    };
    assert_eq!(image.bytes.len(), old_size);
    disk::accept_image(
        &path,
        generation,
        None,
        &image,
        Some(SourceRecord {
            validators,
            rendition: Rendition::Original,
        }),
        &cancel,
    )
    .unwrap()
    .unwrap();
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() - disk::FRESH_AGE * 2)
        .unwrap();
    let baseline = Revalidation {
        path: path.clone(),
        generation,
        receipt: disk::read_entry(&path).unwrap().receipt,
    };
    let original = baseline.receipt.content;
    assert_eq!(baseline.receipt.fresh_for(), Duration::ZERO);
    let decoded = decode::decode(&image).unwrap();
    let original_colors = palette::resolve(
        Some(&store),
        Some((&path, generation)),
        &original,
        &decoded,
        &cancel,
    )
    .unwrap();
    let Refresh::Same(Some(confirmed)) = refresh_with(&source, &baseline, &cancel, fetch) else {
        panic!("304 invalidated image");
    };
    assert_eq!(confirmed.receipt.content, original);
    assert!(confirmed.receipt.fresh_for() > Duration::ZERO);
    let Refresh::Changed(new) = refresh_with(&source, &confirmed, &cancel, fetch) else {
        panic!("changed 200 did not publish");
    };
    assert_eq!(new.image.bytes.len(), new_size);
    assert_ne!(new.content, original);
    let changed_colors = palette::resolve(
        Some(&store),
        Some((&path, generation)),
        &new.content,
        &new.decoded,
        &cancel,
    )
    .unwrap();
    assert_ne!(changed_colors, original_colors);
    assert!(
        store
            .artwork_palette(&new.content, colors::ALGORITHM_VERSION, generation)
            .unwrap()
            .is_some()
    );
    let latest = new.revalidation.unwrap();
    let Refresh::Same(Some(confirmed)) = refresh_with(&source, &latest, &cancel, fetch) else {
        panic!("date validation failed");
    };
    assert_eq!(confirmed.receipt.content, new.content);
    server.join().unwrap();
    assert!(matches!(
        refresh_with(&source, &confirmed, &cancel, fetch),
        Refresh::Failed
    ));
    assert_eq!(disk::read_entry(&path).unwrap().receipt, confirmed.receipt);
}

fn png(pixel: [u8; 4]) -> Vec<u8> {
    let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(64, 64, image::Rgba(pixel)));
    let mut result = Cursor::new(Vec::new());
    image
        .write_to(&mut result, image::ImageFormat::Png)
        .unwrap();
    result.into_inner()
}

#[test]
fn revoked_receipts_cannot_refresh_a_new_blob_or_restore_one_after_clear() {
    let profile = tempfile::tempdir().unwrap();
    let path = disk::path(profile.path(), URL, None).unwrap();
    let generation = disk::generation(&path).unwrap();
    let flag = AtomicBool::new(false);
    let first = Image::from_bytes(gpui::ImageFormat::Png, png([220, 20, 30, 255]));
    let second = Image::from_bytes(gpui::ImageFormat::Png, png([10, 20, 230, 255]));
    let metadata = SourceRecord {
        rendition: Rendition::Original,
        validators: Validators::default(),
    };
    let baseline = disk::accept_image(
        &path,
        generation,
        None,
        &first,
        Some(metadata.clone()),
        &flag,
    )
    .unwrap()
    .unwrap();
    let changed = disk::accept_image(
        &path,
        generation,
        Some(&baseline),
        &second,
        Some(metadata),
        &flag,
    )
    .unwrap()
    .unwrap();
    assert!(
        disk::accept_not_modified(
            &path,
            generation,
            &baseline,
            Validators::default(),
            false,
            &flag
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(disk::read_entry(&path).unwrap().receipt, changed);
    let before = Revalidation {
        path: path.clone(),
        generation,
        receipt: changed.clone(),
    };
    let response = Response::Image {
        image: second,
        validators: Validators::default(),
        no_store: true,
    };
    assert!(matches!(
        apply(&before, Rendition::Original, response, &flag),
        Refresh::Same(None)
    ));
    assert!(disk::read_entry(&path).is_none());
    disk::clear(profile.path()).unwrap();
    assert!(matches!(
        apply(
            &before,
            Rendition::Original,
            Response::Image {
                image: first,
                validators: Validators::default(),
                no_store: false
            },
            &flag
        ),
        Refresh::Failed
    ));
    assert!(disk::read_entry(&path).is_none());
}
