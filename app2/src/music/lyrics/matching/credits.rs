use super::{feature_credit, featured_title, normalized, search_spelling, strip_producer};

#[derive(Clone, Copy)]
pub(crate) struct ArtistCredits<'a> {
    pub display: &'a str,
    pub names: &'a [&'a str],
}

impl<'a> ArtistCredits<'a> {
    pub(crate) fn displayed(display: &'a str) -> Self {
        Self {
            display,
            names: &[],
        }
    }
}

pub(super) fn names(
    credits: ArtistCredits<'_>,
    guests: Option<&str>,
    known: &[&str],
) -> Vec<String> {
    let mut names: Vec<_> = if credits.names.is_empty() {
        displayed_parts(credits.display, known)
    } else {
        credits.names.to_vec()
    }
    .into_iter()
    .chain(
        guests
            .into_iter()
            .flat_map(|guest| guest_parts(guest, known)),
    )
    .map(normalized)
    .filter(|name| !name.is_empty())
    .collect();
    names.sort_unstable();
    names.dedup();
    names
}

fn guest_parts<'a>(value: &'a str, known: &[&str]) -> Vec<&'a str> {
    let known_keys: Vec<_> = known.iter().map(|name| normalized(name)).collect();
    displayed_parts(value, known)
        .into_iter()
        .flat_map(|credit| {
            separate_conjunction(credit, &known_keys).unwrap_or_else(|| vec![credit])
        })
        .collect()
}

fn separate_conjunction<'a>(value: &'a str, known_keys: &[String]) -> Option<Vec<&'a str>> {
    let mut names = Vec::new();
    let mut remaining = value;
    for (offset, _) in value.match_indices(char::is_whitespace) {
        let tail = value[offset..].trim_start();
        if let Some((word, rest)) = tail.split_once(char::is_whitespace)
            && (word.eq_ignore_ascii_case("and") || matches!(word, "и" | "И"))
        {
            let before = &value[..offset];
            let consumed = value.len() - remaining.len();
            if offset > consumed {
                names.push(before[consumed..].trim());
            }
            remaining = rest.trim_start();
        }
    }
    if !names.is_empty() && !known_keys.contains(&normalized(value)) {
        names.push(remaining);
        if names
            .iter()
            .all(|name| known_keys.contains(&normalized(name)))
        {
            return Some(names);
        }
    }
    // A conjunction in an artist's name is a credit separator only when the
    // separately listed participants confirm this rendering of the title.
    None
}

pub(crate) fn displayed_parts<'a>(value: &'a str, known: &[&str]) -> Vec<&'a str> {
    let parts: Vec<_> = credit_parts(value).collect();
    let groups: Vec<_> = known
        .iter()
        .filter_map(|name| {
            let spelling = search_spelling(name).to_lowercase();
            let pieces = credit_parts(&spelling).count();
            (pieces > 1).then_some((spelling, pieces))
        })
        .collect();
    if groups.is_empty() {
        return parts;
    }
    let mut result = Vec::new();
    let mut remaining = parts.as_slice();
    let base = value.as_ptr().addr();
    while let Some(&first) = remaining.first() {
        // The legacy parser returns only subslices of value. Compare the original
        // delimiters within a group, so "A, B" cannot stand in for "A & B".
        let start = first.as_ptr().addr() - base;
        let group = groups
            .iter()
            .filter_map(|(name, pieces)| {
                let last = remaining.get(pieces - 1)?;
                let end = last.as_ptr().addr() - base + last.len();
                let display = &value[start..end];
                (search_spelling(display).to_lowercase() == *name).then_some((display, *pieces))
            })
            .max_by_key(|(_, pieces)| *pieces);
        let (name, used) = group.unwrap_or((first, 1));
        result.push(name);
        remaining = &remaining[used..];
    }
    result
}

pub(super) fn credit_parts(value: &str) -> impl Iterator<Item = &str> {
    let (lead, guests) = featured_title(value);
    [Some(lead), guests]
        .into_iter()
        .flatten()
        .flat_map(|list| list.split([',', ';', '\0']))
        .flat_map(|names| names.split(" & "))
        .flat_map(|names| names.split(" / "))
        .flat_map(|names| {
            let names = names.trim();
            let featured = names
                .match_indices(char::is_whitespace)
                .find_map(|(offset, space)| {
                    feature_credit(&names[offset + space.len()..])
                        .map(|guests| (names[..offset].trim_end(), guests))
                });
            let (lead, guests) =
                featured.map_or((names, None), |(lead, guests)| (lead, Some(guests)));
            [Some(lead), guests].into_iter().flatten()
        })
        .map(strip_producer)
        .filter(|name| !name.is_empty())
}
