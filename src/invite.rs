//! Davet kodu: koordinatörün kimliğinin insan tarafından taşınabilir hali.
//!
//! Kod, koordinatörün public key'idir — kısaltılamaz, çünkü kimliğin kendisi odur.
//! Yapabileceğimiz tek şey onu okunabilir kılmak: hex yerine base32 (64 → 52 karakter),
//! gruplara ayrılmış, ve çözerken biçime karşı hoşgörülü.

use anyhow::{Result, bail};
use data_encoding::BASE32_NOPAD;

/// Gruplandırılmış gösterimde her grubun uzunluğu.
const GROUP: usize = 4;
/// 32 baytın base32 karşılığı.
const CODE_CHARS: usize = 52;

/// 32 baytlık kimliği paylaşılabilir bir koda çevirir.
pub fn encode(key: &[u8; 32]) -> String {
    let raw = BASE32_NOPAD.encode(key).to_lowercase();
    raw.as_bytes()
        .chunks(GROUP)
        .map(|chunk| std::str::from_utf8(chunk).expect("base32 çıktısı ascii"))
        .collect::<Vec<_>>()
        .join("-")
}

/// Kodu kimliğe geri çevirir.
///
/// Kullanıcı kodu WhatsApp'tan kopyalayıp yapıştıracak; bu yüzden tireler, boşluklar,
/// satır sonları ve harf büyüklüğü göz ardı edilir.
pub fn decode(code: &str) -> Result<[u8; 32]> {
    let cleaned: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .flat_map(|c| c.to_uppercase())
        .collect();

    if cleaned.len() != CODE_CHARS {
        bail!(
            "davet kodu {} karakter olmalı, {} karakter geldi",
            CODE_CHARS,
            cleaned.len()
        );
    }

    let bytes = BASE32_NOPAD
        .decode(cleaned.as_bytes())
        .map_err(|_| anyhow::anyhow!("davet kodunda geçersiz karakter var"))?;

    let key: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("davet kodu 32 bayta çözülmedi"))?;
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
        // 52 karakter + 12 tire
        assert_eq!(code.len(), CODE_CHARS + CODE_CHARS / GROUP - 1);
        assert!(code.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert!(code.contains('-'));
        // Hex gösterim 64 karakter olurdu.
        assert!(code.chars().filter(|c| *c != '-').count() < 64);
    }

    /// Kullanıcı kodu nasıl yapıştırırsa yapıştırsın çalışmalı.
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
            assert_eq!(decode(&variant).unwrap(), original, "başarısız: {variant:?}");
        }
    }

    #[test]
    fn rejects_malformed_codes() {
        let valid = encode(&key(3));
        assert!(decode("").is_err(), "boş kod");
        assert!(decode(&valid[..20]).is_err(), "kısa kod");
        assert!(decode(&format!("{valid}aaaa")).is_err(), "uzun kod");
        // '1' ve '8' base32 alfabesinde yok — yazım hatası sessizce kabul edilmemeli.
        let typo = format!("1118{}", &valid[4..]);
        assert!(decode(&typo).is_err(), "geçersiz karakter");
    }
}
