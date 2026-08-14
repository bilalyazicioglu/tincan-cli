//! Oda parolası ile katılım denetimi.
//!
//! Bu katman **şifreleme için değil** — iroh her bağlantıyı QUIC/TLS ile zaten uçtan uca
//! şifreliyor ve karşı tarafın kimliğini public key ile doğruluyor. Parolanın tek işi
//! "bu kodu ele geçiren herkes içeri giremesin" demek.
//!
//! Parola tel üzerinden hiç geçmez: koordinatör rastgele bir nonce yollar, istemci
//! `Argon2id(parola, nonce)` sonucunu geri gönderir. Nonce her bağlantıda yenilendiği
//! için yakalanan bir kanıt tekrar kullanılamaz.

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

/// Parola + nonce'tan kanıt türetir.
///
/// Parolasız odalarda boş dize kullanılır: akış tek yol olsun, "parola var mı" dalı
/// protokolün içine sızmasın diye.
pub fn proof(password: &str, nonce: &Nonce) -> Result<Proof> {
    let mut out = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), nonce, &mut out)
        .map_err(|e| anyhow::anyhow!("anahtar türetme başarısız: {e}"))
        .context("parola kanıtı üretilemedi")?;
    Ok(out)
}

/// Kanıtı sabit zamanda doğrular — zamanlama sızıntısı olmasın diye `==` kullanılmaz.
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
        let p = proof("gizli123", &nonce).unwrap();
        assert!(verify("gizli123", &nonce, &p));
    }

    #[test]
    fn wrong_password_is_rejected() {
        let nonce = random_nonce();
        let p = proof("gizli123", &nonce).unwrap();
        assert!(!verify("gizli124", &nonce, &p));
        assert!(!verify("", &nonce, &p));
    }

    /// Yakalanan bir kanıt başka bir bağlantıda işe yaramamalı.
    #[test]
    fn proof_is_bound_to_its_nonce() {
        let first = random_nonce();
        let second = random_nonce();
        assert_ne!(first, second, "nonce her seferinde yenilenmeli");

        let p = proof("aynı-parola", &first).unwrap();
        assert!(verify("aynı-parola", &first, &p));
        assert!(!verify("aynı-parola", &second, &p), "replay engellenmeli");
    }

    #[test]
    fn passwordless_rooms_work_through_the_same_path() {
        let nonce = random_nonce();
        let p = proof("", &nonce).unwrap();
        assert!(verify("", &nonce, &p));
        assert!(!verify("bir-parola", &nonce, &p));
    }

    #[test]
    fn derivation_is_deterministic() {
        let nonce = [3u8; 16];
        assert_eq!(proof("abc", &nonce).unwrap(), proof("abc", &nonce).unwrap());
    }

    #[test]
    fn non_ascii_passwords_are_supported() {
        let nonce = random_nonce();
        let p = proof("şifreçğü", &nonce).unwrap();
        assert!(verify("şifreçğü", &nonce, &p));
    }
}
