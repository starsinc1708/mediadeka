use std::{fs, path::Path, time::Duration};

use gpui::{AsyncApp, WindowHandle};
use mediadeka::MainView;
use serde_json::json;

use super::super::render_image;
use super::{
    COVER, Scene, draw,
    fixture::{MULTICOLOR_COVER, WARM_COVER},
    require, snapshot, wait,
};

type Colors = [[f32; 3]; 4];
type Regions = [[u32; 4]; 2];
type PixelRegions = [Vec<[u8; 4]>; 2];

const SHADER_PHASE: Duration = Duration::from_millis(4_600);
const TRANSITION: Duration = Duration::from_millis(1_600);
// Clear areas of the upper light field and the lower cool field.
const WIDE_REGIONS: Regions = [[1380, 70, 1690, 153], [1500, 565, 1890, 760]];
const COMPACT_REGIONS: Regions = [[510, 63, 769, 105], [505, 265, 767, 312]];

pub(super) async fn verify_cyan(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_for_capture(Duration::from_secs(3), cx);
        view.set_fluid_glass_cover_for_capture(COVER, cx);
    })?;
    await_ready(window, COVER, cx).await?;
    let mut temperatures = Vec::new();
    for ms in [3000, 3800, 4600] {
        window.update(cx, |view, _, cx| {
            view.sample_fluid_glass_for_capture(Duration::from_millis(ms), cx)
        })?;
        let path = output.join(format!("cover-palette-{ms}.png"));
        draw(raw, scene, Some(&path), cx)?;
        let frame = image::open(path)?.into_rgb8();
        let region = (140..380)
            .step_by(8)
            .flat_map(|y| (1370..1790).step_by(8).map(move |x| (x, y)));
        let (delta, count) = region.fold((0., 0.), |(sum, count), (x, y)| {
            let [r, _, b] = frame.get_pixel(x, y).0;
            (sum + f64::from(r) - f64::from(b), count + 1.)
        });
        temperatures.push(delta / count);
    }
    require(
        temperatures[0] > temperatures[1] && temperatures[1] > temperatures[2] + 4.,
        "Оттенок кэшированной обложки должен плавно сдвинуть фон к синему.",
    )?;
    println!("Cover palette average red minus blue: {temperatures:.2?}");
    Ok(())
}

pub(super) async fn verify_multicolor(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let warm_selected = Duration::from_secs(10);
    let warm = select(window, WARM_COVER, warm_selected, cx).await?;

    let transition_start = warm_selected + TRANSITION;
    let samples = PaletteCapture {
        window,
        scene,
        output,
        regions: WIDE_REGIONS,
    };
    let warm_pixels = samples.capture(transition_start, Some("cover-palette-warm.png"), cx)?;

    let multicolor = select(window, MULTICOLOR_COVER, transition_start, cx).await?;
    let started = samples.capture(transition_start, Some("cover-palette-multi-start.png"), cx)?;
    let middle = samples.capture(
        transition_start + TRANSITION / 2,
        Some("cover-palette-multi-mid.png"),
        cx,
    )?;
    let settled = samples.capture(
        transition_start + TRANSITION,
        Some("cover-palette-multi-settled.png"),
        cx,
    )?;
    let instant = differences(&warm_pixels, &started);
    let halfway = differences(&warm_pixels, &middle);
    let completed = differences(&warm_pixels, &settled);
    let middle_to_settled = differences(&middle, &settled);
    fs::write(
        output.join("cover-palette-roles.json"),
        serde_json::to_vec_pretty(&json!({
            "shader_phase_ms": SHADER_PHASE.as_millis(),
            "roles": ["primary", "secondary", "tertiary", "highlight"],
            "warm_cover_rgb": warm,
            "multicolor_cover_rgb": multicolor,
            "regions_exclusive_right_bottom": WIDE_REGIONS,
            "changed_pixels_from_warm": {"start": instant, "middle": halfway, "settled": completed},
            "changed_pixels_from_middle_to_settled": middle_to_settled,
        }))?,
    )?;
    println!("Palette roles warm {warm:.3?}, multicolor {multicolor:.3?}");
    println!("Palette renderer changes {instant:?} → {halfway:?} → {completed:?}");
    println!("Palette renderer middle to settled {middle_to_settled:?}");

    require(
        [warm[0], multicolor[0]]
            .iter()
            .all(|[r, _, b]| r - b > 0.13),
        "Тёплый основной цвет должен сохраниться при появлении холодного цвета обложки.",
    )?;
    require(
        (1..3).any(|i| {
            let [r, _, b] = multicolor[i];
            b - r > 0.13 && color_distance(multicolor[i], warm[i]) > 0.10
        }),
        "Холодный цвет из многокрасочной обложки должен изменить вторичную или контрастную роль.",
    )?;
    require(
        [(0, 1), (0, 2), (1, 2)]
            .iter()
            .all(|&(a, b)| color_distance(multicolor[a], multicolor[b]) > 0.12),
        "Основная, дополнительная и контрастная роли должны иметь разные цвета.",
    )?;
    require(
        instant == [0; WIDE_REGIONS.len()]
            && halfway.iter().sum::<usize>() > 100
            && completed.iter().sum::<usize>() > 100
            && middle_to_settled.iter().sum::<usize>() > 100,
        "При фиксированной фазе шейдера палитра должна меняться до и после середины перехода.",
    )
}

pub(super) async fn verify_reduced(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let warm_selected = Duration::from_secs(10);
    let multicolor_selected = warm_selected + TRANSITION;
    let later = multicolor_selected + TRANSITION;
    let samples = PaletteCapture {
        window,
        scene,
        output,
        regions: COMPACT_REGIONS,
    };

    select(window, WARM_COVER, warm_selected, cx).await?;
    let warm = samples.capture(warm_selected, Some("cover-palette-warm.png"), cx)?;

    select(window, MULTICOLOR_COVER, multicolor_selected, cx).await?;
    let multicolor = samples.capture(
        multicolor_selected,
        Some("cover-palette-multi-settled.png"),
        cx,
    )?;
    let advanced = samples.capture(later, None, cx)?;
    let immediately_changed = differences(&warm, &multicolor);
    let later_changes = differences(&multicolor, &advanced);
    fs::write(
        output.join("cover-palette-reduced.json"),
        serde_json::to_vec_pretty(&json!({
            "shader_phase_ms": SHADER_PHASE.as_millis(),
            "wall_ms": [warm_selected.as_millis(), multicolor_selected.as_millis(), later.as_millis()],
            "regions_exclusive_right_bottom": COMPACT_REGIONS,
            "changed_pixels_when_ready": immediately_changed,
            "changed_pixels_after_wall_advance": later_changes,
        }))?,
    )?;
    println!(
        "Reduced motion palette changed pixels {immediately_changed:?}, later {later_changes:?}"
    );
    require(
        immediately_changed.iter().sum::<usize>() > 100
            && later_changes == [0; COMPACT_REGIONS.len()],
        "При уменьшенном движении новая палитра должна проявиться сразу и оставаться неподвижной.",
    )
}

async fn select(
    window: WindowHandle<MainView>,
    cover: &str,
    wall: Duration,
    cx: &mut AsyncApp,
) -> gpui::Result<Colors> {
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_palette_for_capture(wall, SHADER_PHASE, cx);
        view.set_fluid_glass_cover_for_capture(cover, cx);
    })?;
    await_ready(window, cover, cx).await
}

async fn await_ready(
    window: WindowHandle<MainView>,
    cover: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<Colors> {
    wait(
        cx,
        "палитра кэшированной обложки",
        |cx| {
            snapshot(window.into(), cx)?;
            window.update(cx, |view, _, cx| {
                view.fluid_glass_palette_ready_for_capture(cover, cx)
                    .then(|| view.fluid_glass_target_colors_for_capture(cx))
            })
        },
    )
    .await
}

struct PaletteCapture<'a> {
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &'a Path,
    regions: Regions,
}

impl PaletteCapture<'_> {
    fn capture(
        &self,
        wall: Duration,
        name: Option<&str>,
        cx: &mut AsyncApp,
    ) -> gpui::Result<PixelRegions> {
        self.window.update(cx, |view, _, cx| {
            view.sample_fluid_glass_palette_for_capture(wall, SHADER_PHASE, cx);
        })?;
        let frame = render_image(self.window.into(), self.scene, cx)?;
        if let Some(name) = name {
            frame.save(self.output.join(name))?;
        }
        Ok(self.regions.map(|[left, top, right, bottom]| {
            (top..bottom)
                .flat_map(|y| (left..right).map(move |x| (x, y)))
                .map(|(x, y)| frame.get_pixel(x, y).0)
                .collect()
        }))
    }
}

fn differences(before: &PixelRegions, after: &PixelRegions) -> [usize; WIDE_REGIONS.len()] {
    std::array::from_fn(|index| {
        before[index]
            .iter()
            .zip(&after[index])
            .filter(|(before, after)| before != after)
            .count()
    })
}

fn color_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.into_iter()
        .zip(b)
        .map(|(l, r)| (l - r).abs())
        .fold(0., f32::max)
}
