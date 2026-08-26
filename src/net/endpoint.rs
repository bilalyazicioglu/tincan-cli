//! iroh endpoint setup and identity conversions.

use anyhow::{Context, Result};
use iroh::address_lookup::MemoryLookup;
use iroh::{Endpoint, EndpointId, RelayMode, endpoint::presets};

use crate::proto::{self, PeerId};

/// Opens the endpoint for the control plane and waits until it is ready.
///
/// `presets::N0` brings in n0's public relays for hole punching plus DNS discovery,
/// which is what makes the invite code sufficient on its own — no address list needed.
pub async fn bind() -> Result<Endpoint> {
    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![proto::ALPN.to_vec(), proto::VOICE_ALPN.to_vec()])
        .relay_mode(RelayMode::Default)
        .bind()
        .await
        .context("could not open the network interface")?;
    endpoint.online().await;
    Ok(endpoint)
}

/// An endpoint for tests: no relays, no discovery, never reaches the outside network.
///
/// This lets the control-plane tests run in milliseconds instead of seconds, without
/// depending on the internet or on n0's servers. Identity alone is not enough to
/// connect here — the full `EndpointAddr` has to be supplied.
pub async fn bind_offline() -> Result<Endpoint> {
    Endpoint::builder(presets::Minimal)
        .alpns(vec![proto::ALPN.to_vec(), proto::VOICE_ALPN.to_vec()])
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .context("could not open the test endpoint")
}

/// For tests: an endpoint with a hand-fed address book instead of a discovery service.
///
/// In the voice mesh peers find each other from their identity, which in production is
/// DNS discovery's job. In tests we fill the book ourselves and switch discovery (and
/// the internet) off.
pub async fn bind_offline_with_lookup() -> Result<(Endpoint, MemoryLookup)> {
    let lookup = MemoryLookup::default();
    let endpoint = Endpoint::builder(presets::Minimal)
        .alpns(vec![proto::ALPN.to_vec(), proto::VOICE_ALPN.to_vec()])
        .relay_mode(RelayMode::Disabled)
        .address_lookup(lookup.clone())
        .bind()
        .await
        .context("could not open the test endpoint")?;
    Ok((endpoint, lookup))
}

pub fn to_peer_id(id: EndpointId) -> PeerId {
    PeerId(*id.as_bytes())
}

pub fn to_endpoint_id(id: &PeerId) -> Result<EndpointId> {
    EndpointId::from_bytes(&id.0).context("invalid peer identity")
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    /// The identity conversion must be lossless — moving between the protocol's raw
    /// bytes and iroh's public key is the foundation the roster rests on.
    #[test]
    fn identity_conversion_round_trips() {
        let key = SecretKey::generate().public();
        let peer = to_peer_id(key);
        assert_eq!(to_endpoint_id(&peer).unwrap(), key);
        assert_eq!(peer.to_string(), key.to_string());
    }

    /// A well-formed invite code that is not a valid curve point — a code with a typo,
    /// say — must produce an error rather than a panic.
    ///
    /// Roughly half of all random 32-byte strings decode to a valid Ed25519 point, so
    /// the test searches for an invalid one instead of hard-coding a single case.
    #[test]
    fn invalid_curve_points_are_rejected_not_panicked_on() {
        let invalid_count = (0u8..=255)
            .filter(|seed| to_endpoint_id(&PeerId([*seed; 32])).is_err())
            .count();
        assert!(
            invalid_count > 0,
            "no byte string was rejected — validation may not be happening"
        );
    }
}
