//! tincan — serverless voice chat that runs in your terminal.
//!
//! The heart of the architecture is the separation of two planes:
//!
//! * **Control plane** (star): whoever opens the room is the coordinator; the roster,
//!   the channels and the chat all flow through them. Small, reliable traffic.
//! * **Voice plane** (mesh): peers in the same channel connect directly to each other
//!   and send Opus packets as unreliable datagrams. Voice never goes through the
//!   coordinator.

pub mod auth;
pub mod clipboard;
pub mod config;
pub mod invite;
pub mod logo;
pub mod proto;
pub mod room;

pub mod net;
pub mod ui;
pub mod audio;
