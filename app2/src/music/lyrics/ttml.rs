use roxmltree::{Document, Node, ParsingOptions};

use super::{Line, Lyrics, MAX_LINE_BYTES, MAX_TEXT_BYTES, Source, Word};

pub(super) fn parse(text: &str) -> Option<Lyrics> {
    if text.len() > MAX_TEXT_BYTES {
        return None;
    }
    let document = Document::parse_with_options(
        text.trim_start_matches('\u{feff}'),
        ParsingOptions {
            nodes_limit: 20_000,
            ..Default::default()
        },
    )
    .ok()?;
    let root = document.root_element();
    if !root.has_tag_name("tt") {
        return None;
    }
    let lines = root
        .descendants()
        .filter(|node| node.has_tag_name("p"))
        .filter_map(line)
        .collect();
    let mut lyrics = Lyrics {
        source: Source::Local,
        lines,
        instrumental: false,
        attribution: None,
    };
    lyrics.prepare();
    lyrics.usable().then_some(lyrics)
}

fn line(node: Node<'_, '_>) -> Option<Line> {
    let time_ms = begin(node)?;
    let mut text = String::new();
    let mut spans = Vec::new();
    collect(node, &mut text, &mut spans, 0)?;
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_LINE_BYTES {
        return None;
    }
    let trim = text.len() - text.trim_start().len();
    let words = spans
        .into_iter()
        .filter_map(|(span, start, end)| {
            let first = span.start.max(trim).min(trim + trimmed.len()) - trim;
            let last = span.end.max(trim).min(trim + trimmed.len()) - trim;
            Word::in_line(trimmed, first..last, start, end)
        })
        .collect();
    Some(Line {
        time_ms: Some(time_ms),
        text: trimmed.to_owned(),
        words,
    })
}

type Span = (std::ops::Range<usize>, u64, Option<u64>);

fn collect(node: Node<'_, '_>, text: &mut String, spans: &mut Vec<Span>, depth: u8) -> Option<()> {
    if depth >= 32 || text.len() > MAX_LINE_BYTES {
        return None;
    }
    for child in node
        .children()
        .filter(|child| child.is_text() || child.is_element())
    {
        if child.is_text() {
            text.push_str(child.text().unwrap_or_default());
        } else if child.has_tag_name("br") {
            text.push('\n');
        } else {
            let first = text.len();
            let existing = spans.len();
            collect(child, text, spans, depth + 1)?;
            if child.has_tag_name("span")
                && spans.len() == existing
                && child.attribute("begin").is_some()
                && let Some(start) = begin(child)
            {
                let end = child
                    .attribute("dur")
                    .and_then(milliseconds)
                    .and_then(|duration| start.checked_add(duration))
                    .or_else(|| {
                        let base = begin(child.parent_element()?)?;
                        base.checked_add(milliseconds(child.attribute("end")?)?)
                    });
                spans.push((first..text.len(), start, end));
            }
        }
    }
    Some(())
}

fn begin(node: Node<'_, '_>) -> Option<u64> {
    node.ancestors()
        .filter_map(|parent| parent.attribute("begin"))
        .try_fold(0_u64, |sum, offset| sum.checked_add(milliseconds(offset)?))
}

fn milliseconds(value: &str) -> Option<u64> {
    let steps: Vec<_> = value.split(':').collect();
    let seconds = match steps.as_slice() {
        [hours, minutes, seconds] => {
            hours.parse::<u64>().ok()? as f64 * 3600.0
                + minutes.parse::<u64>().ok()? as f64 * 60.0
                + seconds.parse::<f64>().ok()?
        }
        [minutes, seconds] => {
            minutes.parse::<u64>().ok()? as f64 * 60.0 + seconds.parse::<f64>().ok()?
        }
        [single] => {
            let (number, factor) = [("ms", 0.001), ("s", 1.0), ("m", 60.0), ("h", 3600.0)]
                .iter()
                .find_map(|(suffix, factor)| {
                    single.strip_suffix(suffix).map(|number| (number, *factor))
                })
                .unwrap_or((single, 1.0));
            number.parse::<f64>().ok()? * factor
        }
        _ => return None,
    };
    (seconds.is_finite() && (0.0..u64::MAX as f64 / 1000.0).contains(&seconds))
        .then(|| (seconds * 1000.0).round() as u64)
}

#[cfg(test)]
mod tests {
    #[test]
    fn ttml_inherits_position_and_keeps_syllable_ranges() {
        let lyrics = super::parse(r#"<tt xmlns="http://www.w3.org/ns/ttml"><body begin="1s"><div><p begin="2s"><span begin="250ms" dur="300ms">🎵</span> <span begin="0.550s">Тест</span></p></div></body></tt>"#).unwrap();
        assert_eq!(lyrics.active_line(2999), None);
        assert_eq!(lyrics.active_line(3000), Some(0));
        assert_eq!(lyrics.lines[0].text, "🎵 Тест");
        let words = &lyrics.lines[0].words;
        assert_eq!(words[0].end_ms, Some(3550));
        assert_eq!(words[1].start_utf16, 3);
    }
}
