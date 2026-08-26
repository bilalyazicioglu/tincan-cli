# 3. Security and Cryptography Model

Tincan is engineered to guarantee privacy, zero-knowledge authentication, and transport confidentiality.

---

## 3.1 Two-Tier Security Architecture

Security is enforced at two distinct layers:

1. **Transport Layer Encryption**: End-to-End Encryption (E2EE) over QUIC TLS 1.3.
2. **Admission Control**: Zero-knowledge challenge-response password authentication.

---

## 3.2 Argon2id Challenge-Response Authentication

When joining a password-protected room:

```
  Joining Peer                                  Coordinator (Host)
       │                                                 │
       ├───────────── QUIC TLS 1.3 Connect ──────────────►│
       │                                                 │
       │◄───────────── 32-Byte Nonce N ──────────────────┤
       │                                                 │
  Compute Proof:                                         │
  K = Argon2id(password, N)                              │
       │                                                 │
       ├───────────── Proof Key K ──────────────────────►│
                                                    Verify Proof:
                                                    Expected = Argon2id(password, N)
                                                    Match -> Grant Entry
```

### Key Security Benefits
- **Zero Password Transmission**: The plain-text password is never sent over the wire.
- **Replay Attack Defense**: Because the host generates a cryptographically random 32-byte nonce $N$ for every join attempt, a captured proof cannot be replayed by eavesdroppers.
- **Argon2id Memory Hardness**: Protects against GPU/ASIC brute-force dictionary attacks.

---

## 3.3 Transport Security & Network Privacy

- **QUIC TLS 1.3**: Every connection is encrypted using QUIC TLS 1.3 backed by Ed25519 public key pairs via Iroh (`src/net/endpoint.rs`).
- **Invite Code = Public Key**: The 63-character invite code is the Base32 representation of the coordinator's public key.
- **DERP Relay Privacy**: When direct P2P hole punching fails, traffic flows through encrypted DERP relays. Relays cannot read audio or text payloads because they lack decryption keys.

---

## 3.4 Threat Model Summary

| Threat | Risk Level | Defense Mechanism |
| :--- | :--- | :--- |
| **Password Interception** | Low | Zero-knowledge Argon2id nonce challenge |
| **Replay Attacks** | Low | Fresh host nonce on every connection attempt |
| **Wire Eavesdropping** | Low | QUIC TLS 1.3 encryption for streams & datagrams |
| **Man-in-the-Middle** | Low | Iroh Ed25519 public key verification |
| **Relay Tampering** | Low | E2E encrypted QUIC payload |

