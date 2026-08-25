//! Room password admission control.
//!
//! This layer is **not encryption** — iroh already encrypts every connection end to end
//! with QUIC/TLS and verifies the other side's identity by public key. The password's
//! only job is to say "not everyone who gets hold of this code may come in".
//!
//! The password never travels over the wire: the coordinator sends a random nonce, and
//! the client returns `Argon2id(password, nonce)`. Because the nonce is regenerated on
//! every connection, a captured proof cannot be replayed.

use anyhow::{Context, Result};
use argon2::Argon2;
use rand::Rng;
use subtle::ConstantTimeEq;

pub type Nonce = [u8; 16];
pub type Proof = [u8; 32];

pub fn random_nonce() -> Nonce {
    let mut nonce = [0u8; 16];
    rand::rng().fill_bytes(&mut nonce);
    nonce
}

/// Derives a proof from a password and a nonce.
///
/// Passwordless rooms use the empty string, so that there is a single path through the
/// handshake and no "is there a password" branch leaks into the protocol.
pub fn proof(password: &str, nonce: &Nonce) -> Result<Proof> {
    let mut out = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), nonce, &mut out)
        .map_err(|e| anyhow::anyhow!("key derivation failed: {e}"))
        .context("could not produce the password proof")?;
    Ok(out)
}

/// Verifies a proof in constant time — `==` is avoided so no timing is leaked.
pub fn verify(password: &str, nonce: &Nonce, presented: &Proof) -> bool {
    match proof(password, nonce) {
        Ok(expected) => expected.ct_eq(presented).into(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correct_password_verifies() {
        let nonce = random_nonce();
        let p = proof("secret123", &nonce).unwrap();
        assert!(verify("secret123", &nonce, &p));
    }

    #[test]
    fn wrong_password_is_rejected() {
        let nonce = random_nonce();
        let p = proof("secret123", &nonce).unwrap();
        assert!(!verify("secret124", &nonce, &p));
        assert!(!verify("", &nonce, &p));
    }

    /// A captured proof must be useless on another connection.
    #[test]
    fn proof_is_bound_to_its_nonce() {
        let first = random_nonce();
        let second = random_nonce();
        assert_ne!(first, second, "the nonce must be fresh every time");

        let p = proof("same-password", &first).unwrap();
        assert!(verify("same-password", &first, &p));
        assert!(!verify("same-password", &second, &p), "replay must be blocked");
    }

    #[test]
    fn passwordless_rooms_work_through_the_same_path() {
        let nonce = random_nonce();
        let p = proof("", &nonce).unwrap();
        assert!(verify("", &nonce, &p));
        assert!(!verify("a-password", &nonce, &p));
    }

    #[test]
    fn derivation_is_deterministic() {
        let nonce = [3u8; 16];
        assert_eq!(proof("abc", &nonce).unwrap(), proof("abc", &nonce).unwrap());
    }

    /// The point of this test is the multi-byte password — keep it non-ASCII.
    #[test]
    fn non_ascii_passwords_are_supported() {
        let nonce = random_nonce();
        let p = proof("パスワードäöü", &nonce).unwrap();
        assert!(verify("パスワードäöü", &nonce, &p));
    }
}
