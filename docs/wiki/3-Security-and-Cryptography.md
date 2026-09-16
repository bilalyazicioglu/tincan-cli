# 3. Security and Cryptography Model

Tincan is engineered to guarantee privacy, zero-knowledge authentication, and transport confidentiality.

---

## 3.1 Two-Tier Security Architecture

Security is enforced at two distinct layers:

1. **Transport Layer Encryption**: End-to-End Encryption (E2EE) over QUIC TLS 1.3.
2. **Admission Control**: Zero-knowledge challenge-response password authentication.

---

## 3.2 Challenge-Response Admission

Both sides stretch the password into an admission key **once**, then prove knowledge of it
per connection with a cheap MAC:

```
  Joining Peer                                  Coordinator (Host)
       │                                                 │
  K = admission key (Argon2id, once)            K = admission key (Argon2id, at startup)
       │                                                 │
       ├───────────── QUIC TLS 1.3 Connect ──────────────►│
       │                                                 │
       │◄───────────── 16-Byte Nonce N ──────────────────┤
       │                                                 │
       ├───────────── Proof = BLAKE2b-MAC(K, N) ─────────►│
                                                    Verify (constant time)
                                                    Match -> Grant Entry
```

How `K` is derived depends on how the room is reached (`src/auth.rs`):

| Way in | Argon2id salt | Cost |
| :--- | :--- | :--- |
| Invite code | `"tincan/invite/v1\0"` ‖ coordinator public key | 19 MiB, 2 passes |
| Room name + passphrase | `"tincan/room/v1\0"` ‖ room name | 64 MiB, 3 passes |

The Argon2id output is a master key; the admission key and (for named rooms) the
coordinator's Ed25519 secret key are separate BLAKE2b subkeys of it. A named room's
coordinator accepts both keys, so its invite code keeps working. Room names and
passphrases are compared case-insensitively, with spaces, dashes and underscores treated
alike. The parameters are pinned in code: they are part of every named room's address.

The control ALPN is `tincan/control/1`; `/0` peers used `Argon2id(password, nonce)` as the
proof and do not interoperate.

### Key Security Benefits
- **Zero Password Transmission**: The plain-text password is never sent over the wire.
- **Replay Attack Defense**: The host generates a fresh random nonce $N$ for every join attempt, so a captured proof cannot be replayed.
- **Argon2id Memory Hardness**: Protects against GPU/ASIC brute-force dictionary attacks.
- **No Per-Connection Argon2 on the Host**: Verifying a proof is a MAC, so connection attempts cannot be used to burn the coordinator's CPU.

---

## 3.3 Transport Security & Network Privacy

- **QUIC TLS 1.3**: Every connection is encrypted using QUIC TLS 1.3 backed by Ed25519 public key pairs via Iroh (`src/net/endpoint.rs`).
- **Invite Code = Public Key**: The 63-character invite code is the Base32 representation of the coordinator's public key.
- **Room Name + Passphrase = Derived Key**: A room opened by name has a coordinator key derived from the two. Its addresses are published through pkarr under that key, and a joiner who derives the same key finds them through the same lookup an invite code uses.
- **Relay Privacy**: When direct P2P hole punching fails, traffic flows through n0's relay servers. A relay cannot read audio or text payloads because it holds no key to the QUIC session, which is established between the two peers rather than with it.
- **Relay Metadata**: A relay does observe the shape of what it forwards — which two public keys are talking, when, and how much. The payload is private; the fact of the conversation is not.
- **Address Exposure**: For a public key to be enough to find an endpoint, iroh publishes that endpoint's reachable addresses — IP addresses and relay URL — as a record signed under the key, and those records are public. Anyone with the invite code can resolve it to an address, as can anyone who derives the same key from a room name and passphrase. Direct connections then reveal each peer's address to every other peer in the room. This is inherent to having no server in the middle, not a defect in the implementation.
- **Dependency on Public Infrastructure**: Discovery (n0's pkarr relay and DNS) and hole punching (n0's relay servers) are third-party services, and tincan does not currently expose a way to substitute your own. Availability, not confidentiality, is what rests on them: without those services peers cannot find each other at all.

---

## 3.4 Threat Model Summary

| Threat | Risk Level | Defense Mechanism |
| :--- | :--- | :--- |
| **Password Interception** | Low | Zero-knowledge Argon2id nonce challenge |
| **Replay Attacks** | Low | Fresh host nonce on every connection attempt |
| **Wire Eavesdropping** | Low | QUIC TLS 1.3 encryption for streams & datagrams |
| **Man-in-the-Middle** | Low | Iroh Ed25519 public key verification |
| **Relay Tampering** | Low | E2E encrypted QUIC payload |
| **IP Address Disclosure to Peers and Code Holders** | Accepted | Inherent to direct connections; use a VPN if the address is the secret |
| **Traffic Metadata at a Relay** | Accepted | Payload is sealed; who-talks-to-whom is not hidden |
| **Loss of n0's Discovery or Relays** | Accepted | Availability only; no fallback or self-hosting yet |
| **Rival Room / Takeover by a Passphrase Holder** (named rooms) | Accepted | Insider attack; use the invite code if it matters |
| **Guessing a Named Room** | Low with a generated passphrase | ~44-bit four-word passphrase behind 64 MiB Argon2id |
