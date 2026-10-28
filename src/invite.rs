//! The invite code: the coordinator's identity in a form a human can carry.
//!
//! The code *is* the coordinator's public key — it cannot be shortened, because it is
//! the identity itself. All we can do is make it readable: base32 instead of hex
//! (64 → 52 characters), split into groups, and forgiving about formatting on the way
//! back in.
//!
//! A room opened by name and passphrase (see `auth`) does not need the code at all; this
//! remains the way in whose security does not rest on a passphrase.

use anyhow::{Context, Result, bail};
use data_encoding::BASE32_NOPAD;
use iroh::RelayUrl;

/// Length of each group in the grouped representation.
const GROUP: usize = 4;
/// The base32 length of 32 bytes.
const CODE_CHARS: usize = 52;

/// Turns a 32-byte identity into a shareable code.
pub fn encode(key: &[u8; 32]) -> String {
    let raw = BASE32_NOPAD.encode(key).to_lowercase();
    raw.as_bytes()
        .chunks(GROUP)
        .map(|chunk| std::str::from_utf8(chunk).expect("base32 output is ascii"))
        .collect::<Vec<_>>()
        .join("-")
}

/// Whether `text` is meant as an invite code rather than a room name.
///
/// Only the shape is checked — 52 ASCII letters and digits once the formatting is gone —
/// so that a code with a typo still lands in `decode` and gets told what is wrong with it,
/// instead of being taken for a room name. Room names are capped below 52 characters, so
/// no room name can pass.
pub fn looks_like_code(text: &str) -> bool {
    let kept: Vec<char> = text.chars().filter(|c| !is_formatting(*c)).collect();
    kept.len() == CODE_CHARS && kept.iter().all(char::is_ascii_alphanumeric)
}

fn is_formatting(c: char) -> bool {
    c.is_whitespace() || c == '-' || c == '_'
}

/// Turns a code back into an identity.
///
/// People copy the code out of a chat app and paste it, so dashes, spaces, line breaks
/// and letter case are all ignored.
pub fn decode(code: &str) -> Result<[u8; 32]> {
    let cleaned: String = code
        .chars()
        .filter(|c| !is_formatting(*c))
        .flat_map(|c| c.to_uppercase())
        .collect();

    if cleaned.len() != CODE_CHARS {
        bail!(
            "an invite code must be {} characters, got {}",
            CODE_CHARS,
            cleaned.len()
        );
    }

    let bytes = BASE32_NOPAD
        .decode(cleaned.as_bytes())
        .map_err(|_| anyhow::anyhow!("the invite code contains an invalid character"))?;

    let key: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("the invite code did not decode to 32 bytes"))?;
    Ok(key)
}

/// What someone typed after `tincan join`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// An invite code: the coordinator's key.
    Code([u8; 32]),
    /// A room opened by name on n0's network.
    Room(String),
    /// A room on a relay its host runs. No name means the relay's default room.
    RelayRoom {
        room: Option<String>,
        relay: RelayUrl,
    },
}

/// Prefix kept for invite forms a later version may add, so that this one can say it is
/// too old instead of taking them for a room name.
const RESERVED_PREFIX: &str = "tincan:";

/// Sorts out what was typed after `join`.
///
/// `name@host` is a room on that relay; a bare host (anything with a dot, a `host:port`,
/// or an `http(s)://` URL) is the relay's default room. That makes a room name with a
/// dot in it a relay address, which is the price of `tincan join relay.example.com`.
pub fn parse_target(input: &str) -> Result<Target> {
    let input = input.trim();
    if input.starts_with(RESERVED_PREFIX) {
        return Err(crate::upgrade::needs_newer("this invite", None).into());
    }
    if looks_like_code(input) {
        return Ok(Target::Code(decode(input)?));
    }
    if input.starts_with("http://") || input.starts_with("https://") {
        return Ok(Target::RelayRoom {
            room: None,
            relay: parse_relay(input)?,
        });
    }
    if let Some((room, host)) = input.rsplit_once('@') {
        if room.trim().is_empty() {
            bail!("{input:?} has no room name before the @");
        }
        let relay = parse_relay(host).with_context(|| format!("could not read {input:?}"))?;
        return Ok(Target::RelayRoom {
            room: Some(room.trim().to_string()),
            relay,
        });
    }
    if !input.contains(char::is_whitespace) && (input.contains('.') || has_port(input)) {
        return Ok(Target::RelayRoom {
            room: None,
            relay: parse_relay(input)?,
        });
    }
    Ok(Target::Room(input.to_string()))
}

fn has_port(input: &str) -> bool {
    input.rsplit_once(':').is_some_and(|(host, port)| {
        !host.is_empty() && !port.is_empty() && port.chars().all(|c| c.is_ascii_digit())
    })
}

/// The invite for a room on a relay, as `parse_target` reads it back: the relay's host
/// when it is served over HTTPS, the full URL otherwise, after the room name and an `@`.
pub fn relay_invite(room: Option<&str>, relay: &RelayUrl) -> String {
    let address = match (relay.scheme(), relay.host_str()) {
        ("https", Some(host)) => match relay.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        },
        _ => relay.as_str().trim_end_matches('/').to_string(),
    };
    match room {
        Some(room) => format!("{room}@{address}"),
        None => address,
    }
}

/// Refuses a room name the host would hand out but `join` would read as something else:
/// on n0's network a name with a dot, an `@` or a port looks like a relay; on a relay, a
/// name that starts like a URL swallows the relay. Better said at `host` than discovered
/// by everyone who tries to join.
pub fn check_room_name(name: &str, relay: Option<&RelayUrl>) -> Result<()> {
    let name = name.trim();
    let (typed, expected) = match relay {
        Some(relay) => (
            relay_invite(Some(name), relay),
            Target::RelayRoom {
                room: Some(name.to_string()),
                relay: relay.clone(),
            },
        ),
        None => (name.to_string(), Target::Room(name.to_string())),
    };
    match parse_target(&typed) {
        Ok(read) if read == expected => Ok(()),
        _ => bail!(
            "{name:?} cannot be a room name: `tincan join` would not read it back as one. \
             leave out dots, @, a :port and a leading http:// or tincan:"
        ),
    }
}

/// Quotes a room name or invite for a `tincan join` line when the shell would split it.
/// `@`, `:` and `/` are plain characters to sh, zsh and fish in a word like these.
pub fn shell_word(word: &str) -> String {
    if word
        .chars()
        .all(|c| c.is_alphanumeric() || "-_.@:/".contains(c))
    {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// What goes on the clipboard: the bare code when that is all it takes, and otherwise
/// the command to run with the passphrase under it.
pub fn to_copy(invite: &str, passphrase: Option<&str>) -> String {
    match passphrase {
        Some(passphrase) => format!(
            "tincan join {}\npassphrase: {passphrase}",
            shell_word(invite)
        ),
        None => invite.to_string(),
    }
}

/// The invite for a room someone joined, as they would hand it on: the same `join`
/// argument, so that F1 on a joiner's screen gives something that works.
pub fn invite_for(target: &Target) -> String {
    match target {
        Target::Code(key) => encode(key),
        Target::Room(name) => name.clone(),
        Target::RelayRoom { room, relay } => relay_invite(room.as_deref(), relay),
    }
}

/// Reads a relay address: a bare host means HTTPS, as every public relay is served.
pub fn parse_relay(input: &str) -> Result<RelayUrl> {
    let input = input.trim();
    if input.is_empty() || input.contains(char::is_whitespace) {
        bail!("{input:?} is not a relay address");
    }
    let url = if input.contains("://") {
        if !(input.starts_with("http://") || input.starts_with("https://")) {
            bail!("{input:?} is not a relay address: it must be http or https");
        }
        input.to_string()
    } else {
        format!("https://{input}")
    };
    let relay: RelayUrl = url
        .parse()
        .map_err(|_| anyhow::anyhow!("{input:?} is not a relay address"))?;
    Ok(relay)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, byte) in k.iter_mut().enumerate() {
            *byte = seed.wrapping_add(i as u8).wrapping_mul(7);
        }
        k
    }

    #[test]
    fn round_trips() {
        for seed in [0u8, 1, 42, 255] {
            let original = key(seed);
            let code = encode(&original);
            assert_eq!(decode(&code).unwrap(), original);
        }
    }

    #[test]
    fn code_is_grouped_and_shorter_than_hex() {
        let code = encode(&key(1));
        // 52 characters + 12 dashes
        assert_eq!(code.len(), CODE_CHARS + CODE_CHARS / GROUP - 1);
        assert!(code.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert!(code.contains('-'));
        // Hex would have been 64 characters.
        assert!(code.chars().filter(|c| *c != '-').count() < 64);
    }

    /// However the user pastes the code, it should work.
    #[test]
    fn decoding_tolerates_user_formatting() {
        let original = key(9);
        let canonical = encode(&original);
        let variants = [
            canonical.replace('-', ""),
            canonical.to_uppercase(),
            format!("  {canonical}\n"),
            canonical.replace('-', " "),
            canonical.replace('-', "_"),
        ];
        for variant in variants {
            assert_eq!(
                decode(&variant).unwrap(),
                original,
                "failed on: {variant:?}"
            );
        }
    }

    #[test]
    fn codes_and_room_names_are_told_apart() {
        let code = encode(&key(5));
        assert!(looks_like_code(&code));
        assert!(looks_like_code(&code.to_uppercase().replace('-', " ")));
        // A typo is still a code, so that decoding can say what is wrong with it.
        assert!(looks_like_code(&format!("1118{}", &code[4..])));

        assert!(!looks_like_code("lobby"));
        assert!(!looks_like_code(&"a".repeat(crate::auth::MAX_ROOM_CHARS)));
        assert!(
            !looks_like_code(&"ş".repeat(CODE_CHARS)),
            "only ASCII can be a code"
        );
    }

    #[test]
    fn rejects_malformed_codes() {
        let valid = encode(&key(3));
        assert!(decode("").is_err(), "empty code");
        assert!(decode(&valid[..20]).is_err(), "short code");
        assert!(decode(&format!("{valid}aaaa")).is_err(), "long code");
        // '1' and '8' are not in the base32 alphabet — a typo must not pass silently.
        let typo = format!("1118{}", &valid[4..]);
        assert!(decode(&typo).is_err(), "invalid character");
    }

    fn relay_room(input: &str) -> (Option<String>, String) {
        match parse_target(input).unwrap() {
            Target::RelayRoom { room, relay } => (room, relay.to_string()),
            _ => panic!("{input:?} should be a relay room"),
        }
    }

    #[test]
    fn a_bare_host_is_a_relay_room() {
        assert_eq!(
            relay_room("relay.example.com"),
            (None, "https://relay.example.com/".into())
        );
    }

    #[test]
    fn a_name_at_a_host_is_a_named_relay_room() {
        assert_eq!(
            relay_room("lobby@relay.example.com"),
            (Some("lobby".into()), "https://relay.example.com/".into())
        );
    }

    #[test]
    fn full_urls_are_kept() {
        assert_eq!(
            relay_room("lobby@http://127.0.0.1:3340"),
            (Some("lobby".into()), "http://127.0.0.1:3340/".into())
        );
    }

    #[test]
    fn a_bare_url_is_a_relay_room() {
        assert_eq!(
            relay_room("http://localhost:3340"),
            (None, "http://localhost:3340/".into())
        );
    }

    #[test]
    fn host_and_port_is_a_relay_room() {
        assert_eq!(
            relay_room("localhost:3340"),
            (None, "https://localhost:3340/".into())
        );
    }

    #[test]
    fn plain_names_and_codes_are_unchanged() {
        assert!(matches!(parse_target("lobby").unwrap(), Target::Room(name) if name == "lobby"));
        let original = key(4);
        assert!(
            matches!(parse_target(&encode(&original)).unwrap(), Target::Code(k) if k == original)
        );
    }

    #[test]
    fn reserved_prefix_asks_for_an_update() {
        let Err(err) = parse_target("tincan:2/abc") else {
            panic!("a reserved invite must not parse");
        };
        assert!(
            err.downcast_ref::<crate::upgrade::NeedsNewerTincan>()
                .is_some()
        );
    }

    #[test]
    fn broken_relay_targets_are_refused() {
        for input in ["@relay.example.com", "lobby@", "lobby@ftp://x.y"] {
            let Err(err) = parse_target(input) else {
                panic!("{input:?} should be refused");
            };
            let message = format!("{err:#}");
            assert!(message.contains(input), "{input:?}: {message}");
        }
    }

    #[test]
    fn relay_invite_is_what_join_reads_back() {
        for relay in ["https://relay.example.com", "http://127.0.0.1:3340"] {
            let relay: RelayUrl = relay.parse().unwrap();
            for room in [None, Some("lobby")] {
                let invite = relay_invite(room, &relay);
                assert_eq!(
                    parse_target(&invite).unwrap(),
                    Target::RelayRoom {
                        room: room.map(str::to_string),
                        relay: relay.clone()
                    },
                    "{invite}"
                );
            }
        }
        let relay: RelayUrl = "https://relay.example.com".parse().unwrap();
        assert_eq!(relay_invite(None, &relay), "relay.example.com");
        assert_eq!(
            relay_invite(Some("lobby"), &relay),
            "lobby@relay.example.com"
        );
    }

    #[test]
    fn a_room_name_join_would_misread_is_refused_by_the_host() {
        for name in ["team.alpha", "bob@home", "clan:42", "tincan:x"] {
            let Err(err) = check_room_name(name, None) else {
                panic!("{name:?} would be read as something else by join");
            };
            assert!(format!("{err:#}").contains(name), "{err:#}");
        }
        assert!(check_room_name("lobby", None).is_ok());
        assert!(check_room_name("game night", None).is_ok());

        let relay: RelayUrl = "https://relay.example.com".parse().unwrap();
        assert!(check_room_name("lobby", Some(&relay)).is_ok());
        assert!(check_room_name("http://x", Some(&relay)).is_err());
        assert!(check_room_name("tincan:x", Some(&relay)).is_err());
    }

    #[test]
    fn the_invite_a_joiner_hands_on_is_what_they_typed() {
        let relay: RelayUrl = "http://localhost:3340".parse().unwrap();
        for target in [
            Target::Room("lobby".into()),
            Target::RelayRoom {
                room: None,
                relay: relay.clone(),
            },
            Target::RelayRoom {
                room: Some("lobby".into()),
                relay,
            },
            Target::Code(key(2)),
        ] {
            assert_eq!(parse_target(&invite_for(&target)).unwrap(), target);
        }
    }
}
