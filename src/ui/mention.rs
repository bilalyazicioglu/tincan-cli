//! Telling whether a message is addressed to you.
//!
//! A mention is `@` and your name as the room shows it, in any case, standing as a
//! word of its own: `@alice,` is one, `@alicebob` and `bob@alice` are not.

use std::ops::Range;

/// Whether `text` calls `name`.
pub fn mentions(text: &str, name: &str) -> bool {
    !ranges(text, name).is_empty()
}

/// Where in `text` each `@name` is, as byte ranges, so the transcript can light them.
pub fn ranges(text: &str, name: &str) -> Vec<Range<usize>> {
    let wanted: Vec<char> = name.trim().chars().flat_map(char::to_lowercase).collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut found = Vec::new();
    for (at, &(start, c)) in chars.iter().enumerate() {
        if c != '@' || (at > 0 && chars[at - 1].1.is_alphanumeric()) {
            continue;
        }
        // Lower-case the text one character at a time and walk it against the name,
        // so a letter whose lower case is longer than itself cannot shift the match.
        let mut want = wanted.iter();
        let mut next = at + 1;
        let mut matched = true;
        'walk: while let Some(&(_, ch)) = chars.get(next) {
            for lower in ch.to_lowercase() {
                match want.next() {
                    Some(&w) if w == lower => {}
                    Some(_) => {
                        matched = false;
                        break 'walk;
                    }
                    None => break 'walk,
                }
            }
            next += 1;
            if want.len() == 0 {
                break;
            }
        }
        if !matched || want.len() != 0 {
            continue;
        }
        if chars
            .get(next)
            .is_some_and(|&(_, after)| after.is_alphanumeric())
        {
            continue;
        }
        let end = chars.get(next).map_or(text.len(), |&(byte, _)| byte);
        found.push(start..end);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn your_name_after_an_at_is_a_mention() {
        for text in ["@alice hi", "hey @Alice!", "@ALICE, come", "end @alice"] {
            assert!(mentions(text, "alice"), "{text:?}");
        }
    }

    #[test]
    fn a_name_inside_another_word_is_not() {
        for text in ["@alicebob hi", "bob@alice", "alice", "@ alice", "@alic"] {
            assert!(!mentions(text, "alice"), "{text:?}");
        }
        assert!(!mentions("@ hi", ""), "an empty name matches nothing");
    }

    #[test]
    fn every_mention_in_a_line_is_found() {
        let text = "@alice and @Alice again, not @alicebob";
        let found: Vec<&str> = ranges(text, "alice")
            .into_iter()
            .map(|range| &text[range])
            .collect();
        assert_eq!(found, ["@alice", "@Alice"]);
    }

    #[test]
    fn names_outside_ascii_are_matched_too() {
        assert!(mentions("@Şule gel", "şule"));
        assert_eq!(ranges("selam @şule", "Şule"), vec![6..12]);
    }
}
