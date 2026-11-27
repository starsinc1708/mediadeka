use std::{env, fs, io::Write, path::Path};

use resvg::{tiny_skia, usvg};

const ICON_SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];
const ICON_MARK_FRACTION: f32 = 0.78;

fn main() {
    println!("cargo:rerun-if-changed=assets");
    let out = env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR");
    let out = Path::new(&out);
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        write_windows_resources(out);
    }
}

fn svg(name: &str) -> usvg::Tree {
    let bytes = fs::read(format!("assets/{name}.svg")).expect("read SVG");
    usvg::Tree::from_data(&bytes, &usvg::Options::default()).expect("parse SVG")
}

fn render_layer(tree: &usvg::Tree, pixmap: &mut tiny_skia::Pixmap, fraction: f32) {
    let side = pixmap.width() as f32;
    let size = tree.size();
    let scale = side * fraction / size.width().max(size.height());
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(
        (side - size.width() * scale) / 2.0,
        (side - size.height() * scale) / 2.0,
    );
    resvg::render(tree, transform, &mut pixmap.as_mut());
}

fn write_windows_resources(out: &Path) {
    let tile = svg("mediadeka-icon-tile");
    let mark = svg("mediadeka");
    let images = ICON_SIZES.map(|side| {
        let mut pixmap = tiny_skia::Pixmap::new(side, side).expect("allocate icon");
        render_layer(&tile, &mut pixmap, 1.0);
        render_layer(&mark, &mut pixmap, ICON_MARK_FRACTION);
        pixmap.encode_png().expect("encode icon")
    });
    let icon_path = out.join("mediadeka.ico");
    let mut ico = fs::File::create(&icon_path).expect("create Windows icon");
    ico.write_all(&[0, 0, 1, 0, ICON_SIZES.len() as u8, 0])
        .expect("write ICO header");
    let mut offset = 6 + 16 * ICON_SIZES.len() as u32;
    for (side, image) in ICON_SIZES.into_iter().zip(&images) {
        let side = side as u8; // The ICO entry convention encodes 256 as zero.
        let len = image.len() as u32;
        ico.write_all(&[side, side, 0, 0, 1, 0, 32, 0])
            .and_then(|()| ico.write_all(&len.to_le_bytes()))
            .and_then(|()| ico.write_all(&offset.to_le_bytes()))
            .expect("write ICO directory");
        offset += len;
    }
    for image in images {
        ico.write_all(&image).expect("write ICO image");
    }
    let rc_file = out.join("mediadeka.rc");
    fs::write(
        &rc_file,
        format!(
            "1 ICON \"{}\"\n",
            icon_path.display().to_string().replace('\\', "/"),
        ),
    )
    .expect("write Windows resource definitions");
    embed_resource::compile(rc_file, embed_resource::NONE)
        .manifest_required()
        .expect("embed Mediadeka icon");
}
