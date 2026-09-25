//! iroh endpoint setup and identity conversions.

use std::time::Duration;

use anyhow::{Context, Result};
use iroh::address_lookup::{
    DEFAULT_PKARR_TTL, EndpointInfo, MemoryLookup, N0_DNS_PKARR_RELAY_PROD, PkarrRelayClient,
};
use iroh::{Endpoint, EndpointId, RelayMode, SecretKey, endpoint::presets};

use tracing::debug;

use crate::proto::{self, PeerId};

/// Opens the endpoint for the control plane and waits until it is ready.
///
/// `presets::N0` brings in n0's public relays for hole punching plus DNS discovery,
/// which is what makes the invite code sufficient on its own — no address list needed.
///
/// `identity` is `None` for a fresh key, which is what joiners and rooms reached by invite
/// code use. A room opened by name passes its derived key instead: the preset's pkarr
/// publisher then announces this endpoint's addresses under it, and a joiner who derives
/// the same key finds them through the same lookup an invite code goes through.
pub async fn bind(identity: Option<SecretKey>) -> Result<Endpoint> {
    let mut builder = Endpoint::builder(presets::N0)
        .alpns(vec![proto::ALPN.to_vec(), proto::VOICE_ALPN.to_vec()])
        .relay_mode(RelayMode::Default);
    if let Some(identity) = identity {
        builder = builder.secret_key(identity);
    }
    let endpoint = builder
        .bind()
        .await
        .context("could not open the network interface")?;
    endpoint.online().await;
    Ok(endpoint)
}

/// Closes the endpoint and takes its address record down with it.
///
/// iroh's pkarr publisher stops when the endpoint does, but leaves its last record up,
/// so a joiner who looks the room up afterwards is sent to an address nobody answers
/// and waits out iroh's 30 s connection timeout before hearing the room is gone.
/// Measured against n0's servers: with an empty record published on the way out the
/// same joiner is told in about 3 s, and a host restarted under the same name and
/// passphrase is reached as quickly as before, because its fresh record replaces this one.
/// The empty record takes a moment to reach n0's lookups: a joiner who asks the instant
/// the host leaves still gets the old one, and in about one run in ten so did one who
/// asked a few seconds later.
///
/// An endpoint with no relay publishes nothing to take down (the offline test
/// endpoints), and one that cannot reach n0 quickly is left as it is: leaving a room
/// never waits on this for long.
pub async fn close_and_retract(endpoint: &Endpoint) {
    const RETRACT_TIMEOUT: Duration = Duration::from_secs(2);

    let publishes = endpoint.addr().relay_urls().next().is_some();
    let client = match (endpoint.dns_resolver(), N0_DNS_PKARR_RELAY_PROD.parse()) {
        (Ok(resolver), Ok(relay)) if publishes => {
            Some(PkarrRelayClient::new(relay, endpoint.tls_config().clone(), resolver.clone()))
        }
        _ => None,
    };
    let secret = endpoint.secret_key().clone();
    endpoint.close().await;

    let Some(client) = client else {
        return;
    };
    let empty = match EndpointInfo::new(secret.public()).to_pkarr_signed_packet(&secret, DEFAULT_PKARR_TTL) {
        Ok(packet) => packet,
        Err(err) => {
            debug!("could not sign the empty address record: {err:#}");
            return;
        }
    };
    match tokio::time::timeout(RETRACT_TIMEOUT, client.publish(&empty)).await {
        Ok(Ok(())) => debug!("address record retracted"),
        Ok(Err(err)) => debug!("could not retract the address record: {err:#}"),
        Err(_) => debug!("gave up retracting the address record"),
    }
}

/// An endpoint for tests: no relays, no discovery, never reaches the outside network.
///
/// This lets the control-plane tests run in milliseconds instead of seconds, without
/// depending on the internet or on n0's servers. Identity alone is not enough to
/// connect here — the full `EndpointAddr` has to be supplied.
pub async fn bind_offline() -> Result<Endpoint> {
    bind_offline_as(SecretKey::generate()).await
}

/// For tests: an offline endpoint with a chosen identity, as a room opened by name has.
pub async fn bind_offline_as(identity: SecretKey) -> Result<Endpoint> {
    Endpoint::builder(presets::Minimal)
        .secret_key(identity)
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
