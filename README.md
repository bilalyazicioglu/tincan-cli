<p align="center">
  <img src="assets/logo.png" width="180" alt="tincan logo">
</p>

<h1 align="center">tincan</h1>

<p align="center">
  <b>Serverless peer-to-peer voice and text chat for your terminal.</b>
</p>

<p align="center">
  <a href="https://github.com/bilalyazicioglu/tincan-cli/actions"><img src="https://img.shields.io/github/actions/workflow/status/bilalyazicioglu/tincan-cli/ci.yml?branch=main&style=flat-square&logo=github&label=build" alt="Build Status"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="License"></a>
  <a href="docs/WIKI.md"><img src="https://img.shields.io/badge/wiki-architecture-orange?style=flat-square" alt="Wiki"></a>
  <a href="https://bilalyazicioglu.com/blog/tincan-terminalde-sesli-sohbet"><img src="https://img.shields.io/badge/blog-developer%20story-purple?style=flat-square" alt="Developer Blog"></a>
</p>

<p align="center">
  tincan does what Discord does, without needing anyone's server: the first person to open the app creates the room, sends the invite code it prints to their friends, and they connect with that code from anywhere in the world. No VPN, no port forwarding, no accounts.
</p>

```
 TINCAN  lobby #general                                       DIRECT  18ms
 CHANNELS                   ⟩ 01:13   bob  hey, bob here
 ▸ ● general             2  │              can you hear me alright?
     gaming              1  │ 01:14 alice  loud and clear
     music                  │ 01:14   bob  oh that is the round trip time
                            │              on the string?
 ON THE LINE · 3            │ 01:15 alice  yes, and the pulse speed is the
 ▁·· alice you     general  │              latency
 ▃▅▇ bob           general  │ 01:15   cem  cem here, joining from gaming
 ▁·· cem            gaming  │ 01:16   bob  nice
                            │ 01:16 alice  it frays when audio drops out
                            │              too
 AUDIO · F6                 │ 01:17   bob  and the meters move with each
 mic  MacBook Pro Microph…  │              voice
 out  AirPods Pro           │ 01:17 alice  that is the whole idea
                            │  #general the names hug the messages now▏
tab channel · f2 talk · f3 mute · f6 audio · ctrl+c     f1 code n73w-kuqc…
```

The palette is metal: a brown tin ground, brass for the string and the invite code,
and the two greens copper actually turns — bright patina for what is live, deeper
verdigris for a link that is holding.

The line down the middle is the connection. It runs taut when everyone is reached
directly, sags into a dashed line when someone is coming through a relay, and frays
when audio starts dropping. While anyone is talking a pulse travels down it, at the
speed of the round trip — so a slow link is something you see rather than read. The
meters beside each name move with that person's voice.

Before anyone has joined, the chat pane draws the same thing full size: your can, the
string, their can, with the latency written on it and your invite code underneath. It
stays up there while the conversation is short enough to leave the room for it, so a
full-screen terminal is never a wall of nothing.

Set `NO_COLOR` for a colourless interface, `TINCAN_ASCII=1` if your terminal has no
box-drawing, `TINCAN_NO_MOTION=1` to hold the string still, and `TINCAN_THEME=light`
for the same palette on a light background.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/bilalyazicioglu/tincan-cli/main/install.sh | sh
```

This downloads a prebuilt binary for your platform into `~/.local/bin` and verifies its
checksum. macOS (Apple Silicon and Intel) and Linux (x86_64 and arm64) are covered. If
you would rather read the script before running it — always a reasonable instinct with
`curl | sh` — it lives at [`install.sh`](install.sh) in this repo.

**From source**, if you prefer it or your platform has no prebuilt binary:

```bash
cargo install --git https://github.com/bilalyazicioglu/tincan-cli
```

That needs Rust 1.91+ and a way to get Opus. The easiest is to install it from your
package manager — `cargo` picks it up through `pkg-config` and links it in:

```bash
brew install opus pkg-config                             # macOS
sudo apt install libopus-dev pkg-config libasound2-dev   # Debian/Ubuntu
```

Without a system Opus, the build compiles the vendored C source instead, which needs
autotools (`autoconf`, `automake`, `libtool`). Either way it takes a few minutes.

However you install it, you need a microphone and speaker that run at 48000 Hz. On the first run your
operating system will ask for microphone permission — on macOS the prompt comes from the
terminal app running tincan (Terminal, iTerm, VS Code…), not from tincan itself.

## Usage

**Open a room:**

```bash
tincan host --name alice --room lobby --password secret
```

It prints an invite code, puts it on your clipboard, and waits for you to press Enter
before taking over the screen. Send it to your friends — copy and paste, 63 characters.

Once the interface is up the footer only has room for the first group of the code. Press
`F1` to print the whole thing into the chat pane and copy it again, so you can invite
someone without restarting the room.

**Join a room:**

```bash
tincan join n73w-kuqc-uog2-... --name bob --password secret
```

**See your audio devices:**

```bash
tincan devices
```

### Options

| Option             | Description                                                    |
| ------------------ | -------------------------------------------------------------- |
| `--name`, `-n`     | Your nickname in the room (default: your system username)      |
| `--password`, `-p` | Room password. Without one, anyone with the code can walk in   |
| `--room`           | Room name (`host` only)                                        |
| `--channels`       | Comma-separated channel list (default: `general,gaming,music`) |
| `--no-voice`       | Skip audio entirely; text chat only                            |
| `--input`          | Microphone to use (a distinctive part of the name is enough)   |
| `--output`         | Speaker to use                                                 |
| `--ptt`            | Push-to-talk: the microphone only opens with F4                |

### Shortcuts

| Key                 | Action                                        |
| ------------------- | --------------------------------------------- |
| `Tab` / `Shift+Tab` | Move between channels                         |
| `F2` (or `Ctrl+G`)  | Join / leave the voice of the channel you see |
| `F3` (or `Ctrl+T`)  | Mute / unmute your microphone                 |
| `F4`                | Push-to-talk (only in `--ptt` mode)           |
| `F5`                | Deafen: hear nobody (also closes your mic)    |
| `F1`                | Show the full invite code (and copy it)       |
| `Enter`             | Send the message                              |
| `Ctrl+C`            | Quit                                          |

To pick a device, list them with `tincan devices` first, then pass part of a name:

```bash
tincan join <code> --input "MacBook Pro Mic" --output "AirPods"
```

The channel you are looking at and the channel you are connected to by voice are
independent: you can read the chat in "general" while talking in "gaming". In the channel
list, `>` marks the one you are viewing and `🔊` the one you are in.

The audio shortcuts are F-keys on purpose: in a terminal `Ctrl+M` (0x0D) and `Ctrl+J`
(0x0A) _are_ Enter and cannot be told apart from it — had those been used, the "mute" key
would have quietly sent a message.

The footer shows link status: how many peers you reach directly, how many flow through a
relay, the worst latency, and whether you have had audio dropouts. When everything is
fine it shows shortcut hints instead — technical detail only surfaces when there is a
problem.

## How it works

Two planes, kept apart:

**Control plane (star).** Whoever opens the room is the _coordinator_: the roster, the
channels and the chat flow through them. The traffic is tiny, a few hundred bytes per
second.

**Voice plane (mesh).** Peers in the same channel connect directly to each other and send
Opus packets as QUIC datagrams. **Voice never passes through the coordinator** — the
host's connection is not a bottleneck, and a six-person room needs about 160 kbps of
upload each.

```
        [alice: coordinator]
         /      |      \          ── control (reliable stream)
      bob     carol    dave
         \______|______/          ── voice (mesh, direct datagrams)
```

Connectivity comes from [iroh](https://iroh.computer): the invite code _is_ the peer's
public key. Most of the time a direct P2P connection is established; if hole punching
fails, traffic flows through a relay — which cannot decrypt anything, it only forwards.
QUIC encrypts every connection end to end and verifies the other side's identity by
public key.

## Security

The password never travels over the wire: the coordinator sends a random nonce and the
client returns `Argon2id(password, nonce)`. Because the nonce is fresh on every
connection, a captured proof cannot be replayed.

The password is not for encryption but for **admission control** — QUIC already handles
the encryption.

> `--password` is visible on the command line, so other users on the same machine can
> read it with `ps`. Keep that in mind on a shared machine.

## Development

```bash
cargo test              # 93 tests: unit + control plane + voice mesh
cargo clippy --all-targets
RUST_LOG=tincan=debug cargo run -- host 2>tincan.log   # logs to a file, so they don't scramble the UI
```

The tests never touch the internet: the control-plane and voice-mesh tests use two real
iroh endpoints over a real QUIC connection, but with relays and discovery disabled and
addresses introduced by hand. The audio tests use no audio hardware either — they attach
directly to the ends of the mesh.

### Source layout

```
src/
  proto.rs        On-the-wire types (control messages + voice packet header)
  room.rs         The room's authoritative state — the coordinator's single source
                  of truth, pure and tested
  auth.rs         Password proof (Argon2id + nonce)
  invite.rs       The invite code: base32, grouped, tolerant of pasting
  net/
    endpoint.rs   iroh endpoint setup, identity conversions
    control.rs    Coordinator server + joining client
    voice.rs      Voice mesh: connection management, datagram transport, channel filter
  audio/
    device.rs     cpal ↔ lock-free ring buffer bridge
    codec.rs      Opus encode/decode + loss concealment
    jitter.rs     Per-peer jitter buffer
    mixer.rs      Multi-source mixing + limiter
    vad.rs        Voice activity detection (indicator + DTX)
  ui/
    state.rs      Interface state (independent of network and terminal, tested)
    view.rs       Screen layout
examples/         Phase 0 probes — throwaway measurement tools
```

`examples/ping.rs` measures connectivity and latency between two machines;
`examples/loopback.rs` measures the audio chain. Both were written to validate design
decisions and are not used in the product.

## Known limits

- **The coordinator is a single point of failure.** If the host leaves, the room
  dissolves. Leader handover was deliberately left out of the MVP.
- **48 kHz required.** There is no resampling; if your device runs at another rate tincan
  says so plainly and falls back to text chat rather than producing broken audio in
  silence.
- **The invite code is 63 characters.** It cannot be shortened, because it is the public
  key itself — fine for copy and paste, not for reading down the phone.
- **Scale is 2–6 people.** In a mesh everyone sends to everyone; past 8 you would need
  the coordinator to mix the audio (an SFU).
- **Push-to-talk is not hold-to-talk.** Terminals generally do not report key-release
  events, so in `--ptt` mode F4 works as a toggle: press once to open the microphone,
  press again to close it.
- **The first second of a connection flows through a relay** before switching to a direct
  link. You may notice the latency in the first moments after joining.

## Contributing

Bug reports and feature requests are welcome! Please use the
[issue templates](.github/ISSUE_TEMPLATE) when opening an issue, and review the
[pull request checklist](.github/PULL_REQUEST_TEMPLATE.md) before submitting a PR.

## License

MIT — see [LICENSE](LICENSE).
