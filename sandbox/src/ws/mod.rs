//! WebSocket authentication ticket subsystem.
//!
//! Provides short-lived, single-use tickets for authenticating the
//! browser WebSocket upgrade handshake.

pub mod fs;
pub mod lsp;
pub mod terminal;
pub mod ticket;
