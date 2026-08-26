//! Per-peer jitter buffer.
//!
//! Network packets arrive at irregular intervals, out of order and incomplete; the
//! sound card, meanwhile, wants one frame every 20 ms and tolerates no delay. This
//! buffer bridges the two: it banks a small amount of latency (60 ms by default) and
//! smooths the stream out.
//!
//! Telling three situations apart is what matters:
//!
//! * **Packet present** → play it.
//! * **Packet lost** (later ones arrived, this one never did) → ask the codec for
//!   concealment (PLC); substituting silence produces an audible click.
//! * **Nobody is talking** → real silence. A peer who is not speaking sends nothing
//!   (DTX), so this is normal and must not be counted as loss.

use std::collections::BTreeMap;

/// A frame coming out of the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// An Opus packet to play.
    Packet(Vec<u8>),
    /// The packet was lost; the codec should conceal it.
    Lost,
    /// The other side is not talking.
    Silence,
}

pub struct JitterBuffer {
    /// How many frames to bank before playback starts.
    target: usize,
    /// Past this depth the latency becomes unacceptable and the buffer fast-forwards.
    max_depth: usize,
    /// The frame expected next. `None` means the stream is paused.
    next_seq: Option<u32>,
    packets: BTreeMap<u32, Vec<u8>>,
    /// How many empty pops in a row — used to decide the stream has stopped.
    starved: usize,
}

/// Consecutive empty frames tolerated before the stream counts as stopped.
const STARVE_LIMIT: usize = 5;

impl JitterBuffer {
    /// `target` is the latency goal in frames (at 20 ms per frame, 3 ≈ 60 ms).
    pub fn new(target: usize) -> Self {
        Self {
            target: target.max(1),
            max_depth: target.max(1) * 4,
            next_seq: None,
            packets: BTreeMap::new(),
            starved: 0,
        }
    }

    pub fn depth(&self) -> usize {
        self.packets.len()
    }

    /// Files an incoming packet. Returns `false` for packets that are too late or
    /// duplicated.
    pub fn push(&mut self, seq: u32, payload: Vec<u8>) -> bool {
        if let Some(next) = self.next_seq
            && seq < next
        {
            // This packet missed its train: its moment has passed, no point keeping it.
            return false;
        }
        if self.packets.contains_key(&seq) {
            return false;
        }
        self.packets.insert(seq, payload);

        // Excess backlog means excess latency. Drop the oldest and fast-forward.
        while self.packets.len() > self.max_depth {
            if let Some(&oldest) = self.packets.keys().next() {
                self.packets.remove(&oldest);
                self.next_seq = Some(oldest + 1);
            }
        }
        true
    }

    /// The next frame to hand to the sound card.
    pub fn pop(&mut self) -> Frame {
        let Some(next) = self.next_seq else {
            // The stream is paused: enough packets must bank up before it restarts.
            if self.packets.len() < self.target {
                return Frame::Silence;
            }
            let first = *self.packets.keys().next().expect("checked to be non-empty");
            self.next_seq = Some(first);
            return self.pop();
        };

        if let Some(payload) = self.packets.remove(&next) {
            self.next_seq = Some(next + 1);
            self.starved = 0;
            return Frame::Packet(payload);
        }

        // The expected packet is missing. If later ones are here, it really was lost.
        if !self.packets.is_empty() {
            self.next_seq = Some(next + 1);
            self.starved = 0;
            return Frame::Lost;
        }

        // The buffer is completely empty: either the network died or they went quiet.
        self.starved += 1;
        if self.starved >= STARVE_LIMIT {
            // Pause the stream; when speech resumes we resynchronise no matter where
            // the sequence number picks up.
            self.next_seq = None;
            self.starved = 0;
        } else {
            self.next_seq = Some(next + 1);
        }
        Frame::Silence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(n: u8) -> Vec<u8> {
        vec![n; 4]
    }

    /// Playback must not start before the buffer fills: that is what the latency
    /// target is for.
    #[test]
    fn waits_until_target_depth_before_playing() {
        let mut buffer = JitterBuffer::new(3);
        buffer.push(0, packet(0));
        assert_eq!(buffer.pop(), Frame::Silence, "must not start on a single packet");
        buffer.push(1, packet(1));
        assert_eq!(buffer.pop(), Frame::Silence);
        buffer.push(2, packet(2));

        assert_eq!(buffer.pop(), Frame::Packet(packet(0)), "must flow once the target is met");
        assert_eq!(buffer.pop(), Frame::Packet(packet(1)));
        assert_eq!(buffer.pop(), Frame::Packet(packet(2)));
    }

    /// Packets that arrive out of order must play in the right order.
    #[test]
    fn reorders_out_of_order_arrivals() {
        let mut buffer = JitterBuffer::new(3);
        buffer.push(2, packet(2));
        buffer.push(0, packet(0));
        buffer.push(1, packet(1));

        assert_eq!(buffer.pop(), Frame::Packet(packet(0)));
        assert_eq!(buffer.pop(), Frame::Packet(packet(1)));
        assert_eq!(buffer.pop(), Frame::Packet(packet(2)));
    }

    /// A lost packet must ask for concealment, not silence — the difference is audible.
    #[test]
    fn reports_loss_when_a_gap_is_surrounded_by_data() {
        let mut buffer = JitterBuffer::new(2);
        buffer.push(0, packet(0));
        buffer.push(2, packet(2));
        buffer.push(3, packet(3));

        assert_eq!(buffer.pop(), Frame::Packet(packet(0)));
        assert_eq!(buffer.pop(), Frame::Lost, "frame 1 is missing");
        assert_eq!(buffer.pop(), Frame::Packet(packet(2)));
        assert_eq!(buffer.pop(), Frame::Packet(packet(3)));
    }

    /// No loss may be reported when the other side goes quiet — with DTX, no packets
    /// arriving is the normal case.
    #[test]
    fn silence_is_not_treated_as_loss() {
        let mut buffer = JitterBuffer::new(2);
        buffer.push(0, packet(0));
        buffer.push(1, packet(1));
        buffer.pop();
        buffer.pop();

        for _ in 0..20 {
            assert_eq!(buffer.pop(), Frame::Silence, "silence must not count as loss");
        }
    }

    /// When speech resumes after a long pause the stream must be picked back up, no
    /// matter where the sequence number continues from.
    #[test]
    fn resynchronises_after_a_long_pause() {
        let mut buffer = JitterBuffer::new(2);
        buffer.push(0, packet(0));
        buffer.push(1, packet(1));
        buffer.pop();
        buffer.pop();
        for _ in 0..STARVE_LIMIT + 2 {
            buffer.pop();
        }

        // Speech restarts much later, at a far higher sequence number.
        buffer.push(900, packet(9));
        buffer.push(901, packet(10));
        assert_eq!(buffer.pop(), Frame::Packet(packet(9)));
        assert_eq!(buffer.pop(), Frame::Packet(packet(10)));
    }

    /// A packet whose moment has passed must not be accepted.
    #[test]
    fn rejects_packets_that_arrive_too_late() {
        let mut buffer = JitterBuffer::new(1);
        buffer.push(5, packet(5));
        buffer.push(6, packet(6));
        assert_eq!(buffer.pop(), Frame::Packet(packet(5)));

        assert!(!buffer.push(5, packet(5)), "a past frame must not be taken back");
        assert_eq!(buffer.pop(), Frame::Packet(packet(6)), "the stream must be undisturbed");
    }

    #[test]
    fn rejects_duplicates() {
        let mut buffer = JitterBuffer::new(2);
        assert!(buffer.push(0, packet(0)));
        assert!(!buffer.push(0, packet(0)), "the same frame must not be taken twice");
        assert_eq!(buffer.depth(), 1);
    }

    /// When the network recovers, the backlog must not turn into latency: the buffer
    /// fast-forwards.
    #[test]
    fn drops_backlog_instead_of_accumulating_delay() {
        let mut buffer = JitterBuffer::new(3);
        for seq in 0..100u32 {
            buffer.push(seq, packet(seq as u8));
        }

        assert!(
            buffer.depth() <= 12,
            "latency must not grow without bound, depth: {}",
            buffer.depth()
        );

        // It must keep flowing properly after fast-forwarding, too.
        assert!(matches!(buffer.pop(), Frame::Packet(_)));
        assert!(matches!(buffer.pop(), Frame::Packet(_)));
    }
}
