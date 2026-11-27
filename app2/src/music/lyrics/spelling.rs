use icu_normalizer::ComposingNormalizer;

/// Normalizes typography for lookup while keeping punctuation used by credit and version parsing.
pub(crate) fn search_spelling(value: &str) -> String {
    let normalized = ComposingNormalizer::new_nfkc().normalize(value);
    let typography: String = normalized
        .chars()
        .filter_map(|character| {
            Some(match character {
                '\u{00ad}' | '\u{200b}' | '\u{2060}' | '\u{feff}' => return None,
                '‐' | '‑' | '‒' | '–' | '—' | '―' => '-',
                '‘' | '’' | 'ʼ' => '\'',
                '“' | '”' | '„' | '«' | '»' => '"',
                character => character,
            })
        })
        .collect();
    typography.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn identity(value: &str) -> String {
    crate::music::normalized(&search_spelling(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_width_composition_and_typography_without_erasing_edition() {
        assert_eq!(search_spelling("Ｔｈｅ　Ｐｈｏｅｎｉｘ"), "The Phoenix");
        assert_eq!(
            search_spelling(" Cafe\u{0301}\u{200b} — “Live” "),
            "Café - \"Live\""
        );
        assert_eq!(identity("ＤＯＮ’Ｔ　ＢＲＥＡＫ"), identity("DON'T BREAK"));
        assert_ne!(identity("The Phoenix (Live)"), identity("The Phoenix"));
    }
}
