//! What a mod's `README.md` shows on its page: the `drh:show` / `drh:hide`
//! zones of the game document.

/// At most this many characters are shown.
pub const MAX_SHOWN_CHARS: usize = 4000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Marker {
    Show,
    EndShow,
    Hide,
    EndHide,
}

/// The shown Markdown, or why the markers are malformed. A page shows the
/// whole file when they are; `--check-mod` refuses it.
pub fn shown(readme: &str) -> Result<String, String> {
    let mut zones: Vec<String> = Vec::new();
    let mut outside = String::new();
    let mut current = String::new();
    let (mut in_show, mut in_hide, mut any_show) = (false, false, false);
    let mut rest = readme;

    loop {
        let Some((before, marker, after)) = next_marker(rest)? else {
            if !in_hide {
                current.push_str(rest);
            }
            break;
        };
        if !in_hide {
            current.push_str(before);
        }
        match marker {
            Marker::Show if in_show || in_hide => {
                return Err("a drh:show marker is inside another zone".to_string());
            }
            Marker::Show => {
                outside.push_str(&std::mem::take(&mut current));
                (in_show, any_show) = (true, true);
            }
            Marker::EndShow if !in_show || in_hide => {
                return Err("a /drh:show marker closes no shown zone".to_string());
            }
            Marker::EndShow => {
                zones.push(std::mem::take(&mut current).trim().to_string());
                in_show = false;
            }
            Marker::Hide if in_hide => {
                return Err("a drh:hide marker is inside a hidden zone".to_string());
            }
            Marker::Hide => in_hide = true,
            Marker::EndHide if !in_hide => {
                return Err("a /drh:hide marker closes no hidden zone".to_string());
            }
            Marker::EndHide => in_hide = false,
        }
        rest = after;
    }
    if in_show || in_hide {
        return Err("a zone is not closed".to_string());
    }
    if any_show {
        zones.retain(|zone| !zone.is_empty());
        return Ok(zones.join("\n\n"));
    }
    outside.push_str(&current);
    Ok(outside.trim().to_string())
}

/// Text before the next `<!-- drh:… -->`, the marker, and the text after it.
fn next_marker(text: &str) -> Result<Option<(&str, Marker, &str)>, String> {
    let mut from = 0;
    while let Some(offset) = text[from..].find("<!--") {
        let start = from + offset;
        let Some(length) = text[start + 4..].find("-->") else {
            return Ok(None);
        };
        let end = start + 4 + length + 3;
        let body = text[start + 4..end - 3].trim();
        if let Some(name) = body.strip_prefix("drh:").or(body.strip_prefix("/drh:")) {
            let closing = body.starts_with('/');
            let marker = match (name.trim(), closing) {
                ("show", false) => Marker::Show,
                ("show", true) => Marker::EndShow,
                ("hide", false) => Marker::Hide,
                ("hide", true) => Marker::EndHide,
                _ => return Err(format!("unknown marker <!-- {body} -->")),
            };
            return Ok(Some((&text[..start], marker, &text[end..])));
        }
        from = end;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_markers_shows_everything() {
        assert_eq!(shown("# Title\n\nBody\n").unwrap(), "# Title\n\nBody");
    }

    #[test]
    fn shown_zones_are_joined_in_order() {
        let readme = "# Title\n<!-- drh:show -->\nOne\n<!-- /drh:show -->\nskipped\n<!--drh:show-->Two<!--/drh:show-->";
        assert_eq!(shown(readme).unwrap(), "One\n\nTwo");
    }

    #[test]
    fn hidden_zones_are_dropped_inside_and_outside_shown_ones() {
        let readme = "A <!-- drh:hide -->secret<!-- /drh:hide --> B";
        assert_eq!(shown(readme).unwrap(), "A  B");
        let readme = "<!-- drh:show -->A<!-- drh:hide -->x<!-- /drh:hide -->B<!-- /drh:show -->";
        assert_eq!(shown(readme).unwrap(), "AB");
    }

    #[test]
    fn an_empty_shown_zone_shows_nothing() {
        assert_eq!(
            shown("# Title\n<!-- drh:show --><!-- /drh:show -->\nMore").unwrap(),
            ""
        );
    }

    #[test]
    fn other_comments_stay() {
        assert_eq!(shown("A <!-- note --> B").unwrap(), "A <!-- note --> B");
    }

    #[test]
    fn malformed_markers_are_refused() {
        for readme in [
            "<!-- drh:show -->A",
            "A<!-- /drh:show -->",
            "<!-- drh:show --><!-- drh:show --><!-- /drh:show --><!-- /drh:show -->",
            "<!-- drh:hide --><!-- drh:show --><!-- /drh:show --><!-- /drh:hide -->",
            "<!-- drh:hide --><!-- drh:hide --><!-- /drh:hide --><!-- /drh:hide -->",
            "<!-- drh:shown -->",
        ] {
            assert!(shown(readme).is_err(), "{readme}");
        }
    }
}
