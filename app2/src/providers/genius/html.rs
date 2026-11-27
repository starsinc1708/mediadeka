pub(super) fn lyrics(mut page: &str) -> Option<String> {
    let mut blocks = Vec::new();
    while let Some((_, rest)) = page.split_once("<div") {
        let Some((attributes, after)) = rest.split_once('>') else {
            break;
        };
        page = after;
        if attribute_true(attributes, "data-lyrics-container") {
            let (text, remaining) = read_container(after)?;
            if !text.is_empty() {
                blocks.push(text);
            }
            page = remaining;
        }
    }
    (!blocks.is_empty()).then(|| blocks.join("\n\n"))
}

fn read_container(mut markup: &str) -> Option<(String, &str)> {
    let mut tags = vec![("div", false)];
    let mut excluded = 0;
    let mut plain = String::new();
    while let Some((text, tag)) = markup.split_once('<') {
        if excluded == 0 {
            decode_into(text, &mut plain);
        }
        if let Some(comment) = tag.strip_prefix("!--") {
            markup = comment.split_once("-->")?.1;
            continue;
        }
        let (tag, after) = tag.split_once('>')?;
        markup = after;
        let (name, attributes) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
        let name = name.trim_end_matches('/');
        if let Some(closing) = name.strip_prefix('/') {
            if let Some(index) = tags.iter().rposition(|(opened, _)| *opened == closing) {
                for (_, hidden) in tags.drain(index..) {
                    if hidden {
                        excluded -= 1;
                    }
                }
            }
            if tags.is_empty() {
                return Some((plain.trim().into(), markup));
            }
        } else if name == "br" {
            if excluded == 0 {
                plain.push('\n');
            }
        } else if matches!(name, "script" | "style" | "template") {
            markup = markup.split_once(&format!("</{name}>"))?.1;
        } else if !tag.ends_with('/')
            && !matches!(name, "img" | "input" | "hr" | "wbr" | "source" | "meta")
        {
            let hidden = attribute_true(attributes, "data-exclude-from-selection");
            if hidden {
                excluded += 1;
            }
            tags.push((name, hidden));
        }
    }
    None
}

fn attribute_true(attributes: &str, name: &str) -> bool {
    attributes.split_whitespace().any(|attribute| {
        attribute
            .split_once('=')
            .is_some_and(|(key, value)| key == name && matches!(value, "\"true\"" | "'true'"))
    })
}

fn decode_into(mut text: &str, output: &mut String) {
    while let Some((prefix, after)) = text.split_once('&') {
        output.push_str(prefix);
        let entity = after.split_once(';').filter(|(name, _)| name.len() <= 12);
        if let Some((name, rest)) = entity
            && let Some(decoded) = decode_entity(name)
        {
            output.push(decoded);
            text = rest;
        } else {
            output.push('&');
            text = after;
        }
    }
    output.push_str(text);
}

fn decode_entity(entity: &str) -> Option<char> {
    Some(match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "apos" => '\'',
        "quot" => '"',
        "nbsp" => ' ',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        reference => {
            let number = reference.strip_prefix('#')?;
            let code = if let Some(hex) = number.strip_prefix(['x', 'X']) {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                number.parse().ok()?
            };
            char::from_u32(code).filter(|value| !value.is_control())?
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_lyric_containers_and_visible_text_are_returned() {
        let page = r#"<div>Promoted</div><div class="Lyrics" data-lyrics-container="true"><div data-exclude-from-selection="true">Header <b>unit</b></div>[Verse]<br />A&amp;B <a>song</a><!-- ignored --><br>&#x041F;&#1086;&#39;&lt;name&gt;<script>no script</script></div><div data-lyrics-container='true'>Outro</div>"#;
        assert_eq!(
            super::lyrics(page).as_deref(),
            Some("[Verse]\nA&B song\nПо'<name>\n\nOutro")
        );
        assert!(super::lyrics("<div>Access denied</div>").is_none());
    }
}
