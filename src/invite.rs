//! The invite code: the coordinator's identity in a form a human can carry.
//!
//! The code *is* the coordinator's public key — it cannot be shortened, because it is
//! the identity itself. All we can do is make it readable: base32 instead of hex
//! (64 → 52 characters), split into groups, and forgiving about formatting on the way
//! back in.

use anyhow::{Result, bail};
use data_encoding::BASE32_NOPAD;

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

/// Turns a code back into an identity.
///
/// People copy the code out of a chat app and paste it, so dashes, spaces, line breaks
/// and letter case are all ignored.
pub fn decode(code: &str) -> Result<[u8; 32]> {
    let cleaned: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
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
            assert_eq!(decode(&variant).unwrap(), original, "failed on: {variant:?}");
        }
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
}
