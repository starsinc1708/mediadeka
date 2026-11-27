use serde::Deserialize;

use crate::music::lyrics::{Line, Lyrics, MAX_LINE_BYTES, MAX_TEXT_BYTES, Source, Word};

#[derive(Deserialize)]
struct LyricsFile {
    lines: Vec<FileLine>,
}

#[derive(Deserialize)]
struct FileLine {
    text: String,
    start_ms: u64,
    #[serde(default)]
    words: Vec<FileWord>,
}

#[derive(Deserialize)]
struct FileWord {
    text: String,
    start_ms: u64,
    end_ms: Option<u64>,
}

pub(super) fn parse(text: &str) -> Option<Lyrics> {
    if text.len() > MAX_TEXT_BYTES {
        return None;
    }
    let parsed: LyricsFile = serde_saphyr::from_str(text).ok()?;
    let lines = parsed.lines.into_iter().filter_map(line).collect();
    let mut lyrics = Lyrics {
        source: Source::Lrclib,
        lines,
        instrumental: false,
        attribution: None,
    };
    lyrics.prepare();
    lyrics.usable().then_some(lyrics)
}

fn line(line: FileLine) -> Option<Line> {
    if line.text.len() > MAX_LINE_BYTES {
        return None;
    }
    let mut words = Vec::new();
    let mut cursor = 0;
    for word in line.words {
        let fragment = word.text.trim();
        if fragment.is_empty() {
            continue;
        }
        let Some(relative) = line.text[cursor..].find(fragment) else {
            // Preserve this line's timing when the word sheet disagrees with it.
            words.clear();
            break;
        };
        let start = cursor + relative;
        cursor = start + fragment.len();
        if let Some(word) = Word::in_line(&line.text, start..cursor, word.start_ms, word.end_ms) {
            words.push(word);
        }
    }
    Some(Line {
        time_ms: Some(line.start_ms),
        text: line.text,
        words,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn lrclib_yaml_word_sync_preserves_unicode() {
        let lyrics = super::parse(
            r#"lines:
- text: "🎵 слово слово"
  start_ms: 100
  words:
  - {text: "🎵", start_ms: 100, end_ms: 200}
  - {text: "слово", start_ms: 220, end_ms: 320}
  - {text: "слово", start_ms: 340, end_ms: 430}
"#,
        )
        .unwrap();
        assert_eq!(lyrics.active_line(100), Some(0));
        let words = &lyrics.lines[0].words;
        assert_eq!((words[1].start_utf16, words[1].end_utf16), (3, 8));
        assert_eq!((words[2].start_utf16, words[2].end_utf16), (9, 14));
    }
}
