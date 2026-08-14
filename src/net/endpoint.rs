//! iroh endpoint kurulumu ve kimlik dönüşümleri.

use anyhow::{Context, Result};
use iroh::{Endpoint, EndpointId, RelayMode, endpoint::presets};

use crate::proto::{self, PeerId};

/// Kontrol düzlemi için endpoint açar ve ağa hazır olmasını bekler.
///
/// `presets::N0` delik açma için n0'ın public relay'lerini ve DNS keşfini getirir;
/// bu sayede davet kodu tek başına (adres listesi olmadan) yeterli oluyor.
pub async fn bind() -> Result<Endpoint> {
    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![proto::ALPN.to_vec()])
        .relay_mode(RelayMode::Default)
        .bind()
        .await
        .context("ağ arayüzü açılamadı")?;
    endpoint.online().await;
    Ok(endpoint)
}

/// Testler için endpoint: relay yok, keşif yok, dış ağa hiç çıkmaz.
///
/// Böylece kontrol düzlemi testleri internete ve n0'ın sunucularına bağımlı olmadan,
/// saniyeler yerine milisaniyelerde koşar. Karşı tarafa bağlanmak için kimlik yetmez,
/// tam `EndpointAddr` verilmelidir.
pub async fn bind_offline() -> Result<Endpoint> {
    Endpoint::builder(presets::Minimal)
        .alpns(vec![proto::ALPN.to_vec()])
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .context("test endpoint'i açılamadı")
}

pub fn to_peer_id(id: EndpointId) -> PeerId {
    PeerId(*id.as_bytes())
}

pub fn to_endpoint_id(id: &PeerId) -> Result<EndpointId> {
    EndpointId::from_bytes(&id.0).context("geçersiz peer kimliği")
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    /// Kimlik dönüşümü kayıpsız olmalı — protokoldeki ham baytlar ile iroh'un
    /// public key'i arasında gidip gelmek roster'ın temeli.
    #[test]
    fn identity_conversion_round_trips() {
        let key = SecretKey::generate().public();
        let peer = to_peer_id(key);
        assert_eq!(to_endpoint_id(&peer).unwrap(), key);
        assert_eq!(peer.to_string(), key.to_string());
    }

    /// Doğru biçimli ama geçerli bir eğri noktası olmayan bir davet kodu — örneğin
    /// yazım hatası içeren bir kod — panikle değil, hatayla karşılanmalı.
    ///
    /// Ed25519'da rastgele 32 baytın kabaca yarısı geçerli bir noktaya çözülür, bu
    /// yüzden test tek bir sabit yerine geçersiz olanı arayarak ilerler.
    #[test]
    fn invalid_curve_points_are_rejected_not_panicked_on() {
        let invalid_count = (0u8..=255)
            .filter(|seed| to_endpoint_id(&PeerId([*seed; 32])).is_err())
            .count();
        assert!(
            invalid_count > 0,
            "hiçbir bayt dizisi reddedilmedi — doğrulama yapılmıyor olabilir"
        );
    }
}
