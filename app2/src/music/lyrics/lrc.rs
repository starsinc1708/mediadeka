use super::{Line, Lyrics, MAX_LINE_BYTES, MAX_TEXT_BYTES, Source, Word};

// Adapted from the Mediadeka v1 LRC parser. Provider and local texts use the
// same timeline, including Enhanced LRC spans and the file's global offset.
pub(crate) fn parse_lrc(source: Source, input: &str) -> Option<Lyrics> {
    if input.len() > MAX_TEXT_BYTES {
        return None;
    }
    let mut offset = 0_i64;
    let mut lines = Vec::new();
    let mut remaining_bytes = MAX_TEXT_BYTES;
    for raw in input.trim_start_matches('\u{feff}').lines() {
        if raw.len() > MAX_LINE_BYTES {
            return None;
        }
        let mut rest = raw.trim();
        if let Some(value) = rest
            .strip_prefix("[offset:")
            .and_then(|tag| tag.strip_suffix(']'))
            .and_then(|value| value.trim().parse::<i64>().ok())
        {
            offset = value;
            continue;
        }
        let mut times = Vec::new();
        while let Some((tag, suffix)) = rest.strip_prefix('[').and_then(|s| s.split_once(']')) {
            let Some(time) = timestamp(tag) else { break };
            times.push(time);
            rest = suffix;
        }
        let Some(&anchor) = times.first() else {
            continue;
        };
        let (text, words) = line_text(rest);
        let required = size_of::<Line>() + text.len() + words.len() * size_of::<Word>();
        for time in times {
            remaining_bytes = remaining_bytes.checked_sub(required)?;
            let shift = i128::from(time) - i128::from(anchor);
            let mut words = words.clone();
            shift_words(&mut words, shift);
            lines.push(Line {
                time_ms: Some(time),
                text: text.clone(),
                words,
            });
        }
    }
    if !lines.iter().any(|line| !line.text.is_empty()) {
        return None;
    }
    let shift = -i128::from(offset);
    for line in &mut lines {
        line.time_ms = line.time_ms.map(|time| shift_time(time, shift));
        shift_words(&mut line.words, shift);
    }
    let mut lyrics = Lyrics {
        source,
        lines,
        instrumental: false,
        attribution: None,
    };
    lyrics.prepare();
    Some(lyrics)
}

fn shift_time(time: u64, shift: i128) -> u64 {
    (i128::from(time) + shift).clamp(0, i128::from(u64::MAX)) as u64
}

fn shift_words(words: &mut Vec<Word>, shift: i128) {
    words.retain_mut(|word| {
        word.time_ms = shift_time(word.time_ms, shift);
        word.end_ms = word.end_ms.map(|end| shift_time(end, shift));
        word.end_ms.is_none_or(|end| end > word.time_ms)
    });
}

fn line_text(mut input: &str) -> (String, Vec<Word>) {
    let mut text = String::new();
    let mut spans = Vec::new();
    let mut active = None;
    while let Some((before, rest)) = input.split_once('<') {
        text.push_str(before);
        let tag_len = rest
            .find(|ch: char| !(ch.is_ascii_digit() || matches!(ch, ':' | '.')))
            .unwrap_or(rest.len());
        let (tag, suffix) = rest.split_at(tag_len);
        if let Some((after, time)) = suffix.strip_prefix('>').zip(timestamp(tag)) {
            if let Some((start, start_time)) = active.replace((text.len(), time)) {
                spans.push((start..text.len(), start_time, Some(time)));
            }
            input = after;
        } else {
            text.push('<');
            text.push_str(tag);
            input = suffix;
        }
    }
    text.push_str(input);
    if let Some((start, time)) = active {
        spans.push((start..text.len(), time, None));
    }
    let trimmed = text.trim();
    let trim_start = text.len() - text.trim_start().len();
    let words = spans
        .into_iter()
        .filter_map(|(span, time_ms, end_ms)| {
            let start = span.start.max(trim_start).min(trim_start + trimmed.len());
            let end = span.end.max(start).min(trim_start + trimmed.len());
            Word::in_line(
                trimmed,
                start - trim_start..end - trim_start,
                time_ms,
                end_ms,
            )
        })
        .collect();
    (trimmed.to_owned(), words)
}

fn timestamp(tag: &str) -> Option<u64> {
    let (minutes, clock) = tag.split_once(':')?;
    let (seconds, fraction) = clock.split_once(['.', ':']).unwrap_or((clock, ""));
    if minutes.is_empty()
        || seconds.len() != 2
        || fraction.len() > 3
        || ![minutes, seconds, fraction]
            .into_iter()
            .all(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let seconds = seconds.parse::<u64>().ok()?;
    if seconds >= 60 {
        return None;
    }
    let fraction_ms = fraction.parse::<u64>().unwrap_or(0) * 10_u64.pow(3 - fraction.len() as u32);
    minutes
        .parse::<u64>()
        .ok()?
        .checked_mul(60_000)?
        .checked_add(seconds * 1000 + fraction_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_cues_offsets_and_utf16_word_ranges_follow_seek() {
        let lyrics = parse_lrc(
            Source::Local,
            "[00:01.00][00:04.00] <00:01.00>🎵 <00:01.55>Слово<00:02.30>\n[offset:100]",
        )
        .unwrap();
        assert_eq!(lyrics.active_line(899), None);
        assert_eq!(lyrics.active_line(900), Some(0));
        assert_eq!(lyrics.active_line(3900), Some(1));
        assert_eq!(lyrics.lines[0].text, "🎵 Слово");
        assert_eq!(lyrics.lines[0].words[1].start_utf16, 3);
        assert_eq!(lyrics.lines[0].words[1].end_utf16, 8);
        assert_eq!(lyrics.lines[1].words[1].time_ms, 4450);
        assert_eq!(lyrics.lines[1].words[1].end_ms, Some(5200));
    }
}
