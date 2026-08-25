//! Networking layer: the iroh endpoint, the control plane and the voice mesh.

pub mod control;
pub mod endpoint;

use tokio::sync::mpsc;

use crate::proto::{ChannelId, ChatLine, PeerId, PeerInfo, RoomSnapshot};

/// User actions coming from the interface.
///
/// The host and the joiner send the same commands; the only difference is which
/// constructor built the `Session`. The interface never has to know who is hosting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    SwitchChannel(Option<ChannelId>),
    /// The channel travels explicitly: the user can switch channels while typing, and
    /// the message must land in the channel it was written in.
    Chat { channel: ChannelId, text: String },
    SetMuted(bool),
    SetDeafened(bool),
    Quit,
}

/// Events going out to the interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Welcome { me: PeerId, room: RoomSnapshot },
    Roster(Vec<PeerInfo>),
    Chat(ChatLine),
    Notice(String),
    /// The session is over — the coordinator shut down, the link dropped, or we were
    /// rejected.
    Disconnected(String),
}

/// Where the interface holds on to the control plane.
pub struct Session {
    pub me: PeerId,
    /// The invite code — our own when hosting, the room we joined otherwise.
    pub invite_code: String,
    pub commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
}

/// The clock the coordinator uses to order chat.
pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
pub mod voice;
