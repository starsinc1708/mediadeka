//! Weighted median cut over a bounded sample of the decoded cover.

use gpui::RenderImage;

pub(super) const SAMPLE_EDGE: usize = 64;
pub(super) const ALGORITHM_VERSION: i64 = 1;
const MAX_SWATCHES: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::ui) struct ArtworkColors {
    swatches: [(u32, f32); MAX_SWATCHES],
    len: usize,
}

pub(super) fn encode(colors: Option<ArtworkColors>) -> Vec<u8> {
    let swatches = colors.as_ref().map_or(&[][..], ArtworkColors::swatches);
    let mut bytes = Vec::with_capacity(1 + swatches.len() * 8);
    bytes.push(swatches.len() as u8);
    for &(rgb, population) in swatches {
        bytes.extend_from_slice(&rgb.to_le_bytes());
        bytes.extend_from_slice(&population.to_le_bytes());
    }
    bytes
}

pub(super) fn decode(bytes: &[u8]) -> Option<Option<ArtworkColors>> {
    let (&len, bytes) = bytes.split_first()?;
    let len = usize::from(len);
    if len > MAX_SWATCHES || bytes.len() != len * 8 {
        return None;
    }
    if len == 0 {
        return Some(None);
    }
    let mut swatches = [(0, 0.); MAX_SWATCHES];
    for (slot, &[r0, r1, r2, r3, p0, p1, p2, p3]) in
        swatches.iter_mut().zip(bytes.as_chunks::<8>().0)
    {
        let rgb = u32::from_le_bytes([r0, r1, r2, r3]);
        let population = f32::from_le_bytes([p0, p1, p2, p3]);
        if rgb > 0x00ff_ffff || !(population > 0. && population <= 1.) {
            return None;
        }
        *slot = (rgb, population);
    }
    let active = &swatches[..len];
    let sum = active.iter().map(|&(_, share)| share).sum::<f32>();
    if (sum - 1.).abs() > 0.001 || active.windows(2).any(|pair| pair[0].1 < pair[1].1) {
        return None;
    }
    Some(Some(ArtworkColors { swatches, len }))
}

impl ArtworkColors {
    /// sRGB colors and shares of visible pixel mass, in descending population order.
    pub(in crate::ui) fn swatches(&self) -> &[(u32, f32)] {
        &self.swatches[..self.len]
    }
}

pub(super) fn extract(image: &RenderImage) -> Option<ArtworkColors> {
    let bytes = image.as_bytes(0)?;
    let size = image.size(0);
    let width = usize::try_from(size.width.0).ok()?;
    let height = usize::try_from(size.height.0).ok()?;
    let (pixels, trailing) = bytes.as_chunks::<4>();
    if width == 0
        || height == 0
        || !trailing.is_empty()
        || width.checked_mul(height)? != pixels.len()
    {
        return None;
    }

    let mut samples = sample_pixels(pixels, width, height);
    let root = Bucket::measure(&samples, 0, samples.len());
    if root.mass == 0 {
        return None;
    }

    let mut buckets = Vec::with_capacity(MAX_SWATCHES);
    buckets.push(root);
    while buckets.len() < MAX_SWATCHES {
        let Some((index, &bucket)) = buckets
            .iter()
            .enumerate()
            .filter(|(_, bucket)| bucket.spread != 0)
            .max_by_key(|(_, bucket)| u32::from(bucket.spread) * bucket.mass)
        else {
            break;
        };
        let Some((left, right)) = bucket.split(&mut samples) else {
            break;
        };
        buckets[index] = left;
        buckets.push(right);
    }

    let mut swatches = [(0, 0.); MAX_SWATCHES];
    for (swatch, bucket) in swatches.iter_mut().zip(&buckets) {
        *swatch = (
            bucket.average(&samples),
            bucket.mass as f32 / root.mass as f32,
        );
    }
    let len = buckets.len();
    swatches[..len].sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Some(ArtworkColors { swatches, len })
}

struct Sample {
    rgb: [u8; 3],
    alpha: u8,
}

fn sample_pixels(pixels: &[[u8; 4]], width: usize, height: usize) -> Vec<Sample> {
    let columns = width.min(SAMPLE_EDGE);
    let rows = height.min(SAMPLE_EDGE);
    let mut samples = Vec::with_capacity(columns * rows);
    for row in 0..rows {
        for column in 0..columns {
            // Vary the position within each cell so repeating patterns do not alias to one color.
            let seed = ((row * columns + column) as u32).wrapping_mul(0x9e37_79b9);
            let x = sample_coordinate(column, columns, width, (seed ^ (seed >> 16)) as usize);
            let y = sample_coordinate(row, rows, height, (seed >> 16) as usize);
            let [b, g, r, alpha] = pixels[y * width + x];
            if alpha != 0 {
                samples.push(Sample {
                    rgb: [r, g, b],
                    alpha,
                });
            }
        }
    }
    samples
}

fn sample_coordinate(cell: usize, cells: usize, extent: usize, seed: usize) -> usize {
    let boundary = |part| (part as u64 * extent as u64 / cells as u64) as usize;
    let start = boundary(cell);
    let end = boundary(cell + 1);
    start + seed % (end - start)
}

#[derive(Clone, Copy)]
struct Bucket {
    begin: usize,
    end: usize,
    mass: u32,
    axis: usize,
    spread: u8,
}

impl Bucket {
    fn measure(samples: &[Sample], begin: usize, end: usize) -> Self {
        let mut smallest = [u8::MAX; 3];
        let mut largest = [u8::MIN; 3];
        let mut mass = 0;
        for sample in &samples[begin..end] {
            mass += u32::from(sample.alpha);
            for (axis, &channel) in sample.rgb.iter().enumerate() {
                smallest[axis] = smallest[axis].min(channel);
                largest[axis] = largest[axis].max(channel);
            }
        }
        let spreads = std::array::from_fn::<_, 3, _>(|i| largest[i].saturating_sub(smallest[i]));
        let axis = if spreads[0] >= spreads[1].max(spreads[2]) {
            0
        } else {
            1 + usize::from(spreads[2] > spreads[1])
        };
        Self {
            begin,
            end,
            mass,
            axis,
            spread: spreads[axis],
        }
    }

    fn split(self, samples: &mut [Sample]) -> Option<(Self, Self)> {
        let colors = &mut samples[self.begin..self.end];
        colors.sort_unstable_by_key(|sample| sample.rgb[self.axis]);
        let mut left_mass = 0_u32;
        let (cut, _) = colors
            .windows(2)
            .enumerate()
            .filter_map(|(index, pair)| {
                left_mass += u32::from(pair[0].alpha);
                let gap = left_mass.abs_diff(self.mass - left_mass);
                (pair[0].rgb[self.axis] != pair[1].rgb[self.axis]).then_some((index + 1, gap))
            })
            .min_by_key(|&(_, gap)| gap)?;
        let cut = self.begin + cut;
        Some((
            Self::measure(samples, self.begin, cut),
            Self::measure(samples, cut, self.end),
        ))
    }

    fn average(self, samples: &[Sample]) -> u32 {
        let mut sum = [0; 3];
        for sample in &samples[self.begin..self.end] {
            for (total, value) in sum.iter_mut().zip(sample.rgb) {
                *total += u32::from(value) * u32::from(sample.alpha);
            }
        }
        let [r, g, b] = sum.map(|sum| (sum + self.mass / 2) / self.mass);
        (r << 16) | (g << 8) | b
    }
}

#[cfg(test)]
mod tests {
    use image::{Frame, Rgba, RgbaImage};

    use super::*;

    fn image(width: u32, height: u32, pixel: impl Fn(u32, u32) -> (u32, u8)) -> RenderImage {
        RenderImage::new([Frame::new(RgbaImage::from_fn(width, height, |x, y| {
            let (color, alpha) = pixel(x, y);
            let [_, r, g, b] = color.to_be_bytes();
            Rgba([b, g, r, alpha])
        }))])
    }

    #[test]
    fn preserves_six_cover_colors_and_orders_them_by_population() {
        let colors = [0xd74224, 0xeab467, 0xead9aa, 0x113ca0, 0x217898, 0x25133a];
        let image = image(60, 10, |x, _| {
            let index = [20, 33, 43, 51, 56]
                .iter()
                .position(|&edge| x < edge)
                .unwrap_or(5);
            (colors[index], 255)
        });
        let palette = extract(&image).unwrap();
        let swatches = palette.swatches();
        assert_eq!(swatches.len(), colors.len());
        for (swatch, color) in swatches.iter().zip(colors) {
            assert_eq!(swatch.0, color);
        }
        assert_eq!(swatches[0].1, 20. / 60.);
        assert!((swatches.iter().map(|swatch| swatch.1).sum::<f32>() - 1.).abs() < f32::EPSILON);
    }

    #[test]
    fn neutral_pixels_and_alpha_keep_their_own_populations() {
        let image = image(4, 1, |x, _| {
            [
                (0x303030, 255),
                (0xeeeeee, 128),
                (0xff0000, 0),
                (0x0000ff, 0),
            ][x as usize]
        });
        let palette = extract(&image).unwrap();
        assert_eq!(
            palette.swatches(),
            &[(0x303030, 255. / 383.), (0xeeeeee, 128. / 383.)]
        );
    }

    #[test]
    fn downsampling_preserves_both_colors_in_fine_patterns() {
        let dark = 0x231629;
        let light = 0x9d61be;
        let image = image(128, 128, |x, y| {
            (if (x + y) % 2 == 0 { dark } else { light }, 255)
        });
        let palette = extract(&image).unwrap();
        assert_eq!(palette.swatches().len(), 2);
        assert!(palette.swatches().iter().all(|&(color, mass)| {
            matches!(color, 0x231629 | 0x9d61be) && (0.46..0.54).contains(&mass)
        }));
    }

    #[test]
    fn missing_and_fully_transparent_covers_have_no_palette() {
        assert_eq!(extract(&RenderImage::new(Vec::<Frame>::new())), None);
        assert_eq!(extract(&image(4, 2, |_, _| (0xff0000, 0))), None);
    }
}
