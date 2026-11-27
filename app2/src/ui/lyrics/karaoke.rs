use std::ops::Range;

use gpui::{
    AnyElement, Bounds, ContentMask, HighlightStyle, Hsla, IntoElement, LayerFilter, Pixels,
    SharedString, StyledText, WrappedLine, canvas, div, prelude::*, px,
};

use crate::music::lyrics::Line;

use super::clock::word_bytes;

const LAST_WORD_MS: u64 = 500;

#[derive(Debug, PartialEq)]
pub(super) struct Sweep {
    finished: usize,
    partial: Option<(Range<usize>, f32)>,
}

impl Sweep {
    pub(super) fn at(line: &Line, position: u64, next_line: Option<u64>, discrete: bool) -> Self {
        let Some(index) = line.words.iter().rposition(|word| word.time_ms <= position) else {
            return Self {
                finished: 0,
                partial: None,
            };
        };
        let word = &line.words[index];
        let next_word = line.words.get(index + 1).map(|word| word.time_ms);
        let end = word
            .end_ms
            .or(next_word)
            .filter(|end| *end > word.time_ms)
            .unwrap_or_else(|| {
                let fallback = word.time_ms.saturating_add(LAST_WORD_MS);
                next_line
                    .filter(|end| *end > word.time_ms)
                    .map_or(fallback, |end| end.min(fallback))
            });
        let Some(range) = word_bytes(line, index) else {
            return Self {
                finished: 0,
                partial: None,
            };
        };
        if discrete || position >= end {
            return Self {
                finished: range.end,
                partial: None,
            };
        }

        let progress = (position - word.time_ms) as f32 / (end - word.time_ms) as f32;
        Self {
            finished: range.start,
            partial: Some((range, progress)),
        }
    }

    pub(super) fn moving(&self) -> bool {
        self.partial.is_some()
    }
}

pub(super) fn text(label: SharedString, sweep: Option<Sweep>, tint: Hsla) -> AnyElement {
    let highlights = sweep
        .as_ref()
        .filter(|sweep| sweep.finished != 0)
        .map(|sweep| {
            (
                0..sweep.finished,
                HighlightStyle {
                    color: Some(tint),
                    ..Default::default()
                },
            )
        });
    let base = StyledText::new(label.clone()).with_highlights(highlights);
    let Some((word, progress)) = sweep
        .and_then(|sweep| sweep.partial)
        .filter(|(_, progress)| *progress > 0.)
    else {
        return base.into_any_element();
    };
    let text_layout = base.layout().clone();
    let paint_word = word.clone();
    let overlay = canvas(
        move |_, window, _| {
            let style = window.text_style();
            let shaped = text_layout.line_layouts();
            let wrap = shaped.first()?.wrap_width;
            let layout = Overlay {
                bounds: text_layout.bounds(),
                height: text_layout.line_height(),
                align: style.text_align,
                font_size: style.font_size.to_pixels(window.rem_size()),
                tint,
            };
            // GPUI does not expose the rearranged screen coordinates of wrapped bidi rows.
            // Let its decoration runs color the timed span in that layout.
            let crossfade = shaped.iter().any(|line| {
                !line.wrap_boundaries().is_empty() && !line.unwrapped_layout.reads_left_to_right()
            });
            let run = style.to_run(label.len());
            let runs = if crossfade {
                vec![
                    gpui::TextRun {
                        len: word.start,
                        color: tint.alpha(0.),
                        ..run.clone()
                    },
                    gpui::TextRun {
                        len: word.len(),
                        color: tint.alpha(progress),
                        ..run.clone()
                    },
                    gpui::TextRun {
                        len: label.len() - word.end,
                        color: tint.alpha(0.),
                        ..run
                    },
                ]
            } else {
                vec![run]
            };
            let lines = window
                .text_system()
                .shape_text(label.clone(), layout.font_size, &runs, wrap, None)
                .ok()?;
            Some((layout, lines, crossfade))
        },
        move |_, prepared, window, cx| {
            if let Some((layout, lines, crossfade)) = prepared {
                layout.paint(lines, paint_word.clone(), progress, crossfade, window, cx);
            }
        },
    )
    .absolute()
    .inset_0();
    div()
        .relative()
        .min_w_0()
        .child(base)
        .child(overlay)
        .into_any_element()
}

struct Overlay {
    bounds: Bounds<Pixels>,
    height: Pixels,
    align: gpui::TextAlign,
    font_size: Pixels,
    tint: Hsla,
}

impl Overlay {
    fn paint(
        &self,
        lines: impl IntoIterator<Item = WrappedLine>,
        word: Range<usize>,
        progress: f32,
        crossfade: bool,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        let mut byte = 0;
        let mut origin = self.bounds.origin;
        for line in lines {
            let end = byte + line.len();
            let start = word.start.max(byte);
            let stop = word.end.min(end);
            if start < stop {
                if crossfade {
                    let _ = line.paint(
                        origin,
                        self.height,
                        self.align,
                        Some(self.bounds),
                        window,
                        cx,
                    );
                } else {
                    let parts = parts(&line, (start - byte)..(stop - byte), self.height);
                    let width: Pixels = parts.iter().map(Part::width).sum();
                    let mut remaining = width * progress;
                    for part in parts {
                        let shown = remaining.min(part.width());
                        if shown > px(0.) {
                            self.paint_fragment(&line, origin, &part, shown, window, cx);
                        }
                        remaining -= shown;
                    }
                }
            }
            byte = end + 1;
            origin.y += line.size(self.height).height;
        }
    }

    fn paint_fragment(
        &self,
        line: &WrappedLine,
        origin: gpui::Point<Pixels>,
        part: &Part,
        shown: Pixels,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        let left = if part.rtl {
            part.right - shown
        } else {
            part.left
        };
        let clip = Bounds {
            origin: origin + gpui::point(left, part.y),
            size: gpui::size(shown, self.height),
        };
        let fade = (self.font_size * 0.24)
            .min(shown)
            .min((part.width() - shown) * 0.8);
        let filter = LayerFilter {
            fade_left: (part.rtl && fade > px(0.)).then_some(fade),
            fade_right: (!part.rtl && fade > px(0.)).then_some(fade),
            ..Default::default()
        };
        window.with_filter(clip, filter, px(0.), |window| {
            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                let _ = line.paint_tinted(
                    origin,
                    self.height,
                    self.align,
                    Some(self.bounds),
                    self.tint,
                    window,
                    cx,
                );
            });
        });
    }
}

struct Part {
    left: Pixels,
    right: Pixels,
    y: Pixels,
    rtl: bool,
}

impl Part {
    fn width(&self) -> Pixels {
        self.right - self.left
    }
}

fn parts(line: &WrappedLine, word: Range<usize>, height: Pixels) -> Vec<Part> {
    let shape = &line.unwrapped_layout;
    let ends = line
        .wrap_boundaries()
        .iter()
        .map(|boundary| shape.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index)
        .chain([line.len()]);
    let mut row_start = 0;
    let mut parts = Vec::new();
    for (row, row_end) in ends.enumerate() {
        let start = word.start.max(row_start);
        let end = word.end.min(row_end);
        if start < end {
            let row_left = if row == 0 {
                px(0.)
            } else {
                shape.x_for_index(row_start)
            };
            let from = shape.x_for_index(start) - row_left;
            let to = shape.x_for_index(end) - row_left;
            parts.push(Part {
                left: from.min(to).max(px(0.)),
                right: from.max(to).max(px(0.)),
                y: height * row,
                rtl: from > to,
            });
        }
        row_start = row_end;
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::lyrics::Word;

    #[test]
    fn renders_a_partial_word_without_losing_the_completed_prefix() {
        let text = "🎵 Вперёд снова";
        let split = text.find("снова").unwrap();
        let line = Line {
            time_ms: Some(1000),
            text: text.to_owned(),
            words: vec![
                Word::in_line(text, 0..split, 1000, Some(1300)).unwrap(),
                Word::in_line(text, split..text.len(), 1500, Some(2500)).unwrap(),
            ],
        };
        let progress = Sweep::at(&line, 1750, None, false);
        assert_eq!(progress.finished, split);
        assert_eq!(progress.partial, Some((split..text.len(), 0.25)));
        assert!(Sweep::at(&line, 1400, None, false).partial.is_none());
        assert_eq!(Sweep::at(&line, 1750, None, true).finished, text.len());
        assert_eq!(Sweep::at(&line, 2500, None, false).finished, text.len());
    }

    #[test]
    fn untimed_word_end_does_not_stretch_across_an_instrumental_gap() {
        let text = "Тишина";
        let line = Line {
            time_ms: Some(1000),
            text: text.to_owned(),
            words: vec![Word::in_line(text, 0..text.len(), 1000, None).unwrap()],
        };
        assert!(Sweep::at(&line, 1250, Some(20_000), false).moving());
        assert!(!Sweep::at(&line, 1500, Some(20_000), false).moving());
        assert!(!Sweep::at(&line, 1180, Some(1180), false).moving());
    }
}
