//! JSONL RPC client for `pi --mode rpc` — the transport under reactor-gui.
//!
//! The wire protocol is pi's documented RPC mode: JSON objects on stdin,
//! responses and streamed events on stdout, one record per line. See
//! [`gui/SPEC.md`](../../gui/SPEC.md) §2 for why this is the transport and
//! `docs/pi-api-notes.md` for the pi 0.87.0 facts it rests on.
//!
//! Three pieces:
//! - [`framing`] — the LF-only record split (U+2028-safe by construction)
//! - [`protocol`] — typed views of responses, events, and the extension-UI
//!   sub-protocol, tolerant of unknown shapes by design
//! - [`client`] — spawn/connect, correlation, and typed command helpers
//!
//! Tests are hermetic: the routing rules run against fixture lines with no
//! child process, and the live-pi round trips are `#[ignore]`d.

pub mod client;
pub mod error;
pub mod framing;
pub mod protocol;

pub use client::{Command, Incoming, RequestError, RpcClient, SpawnConfig};
pub use error::{Error, Result};
pub use protocol::{
    AssistantMessageEvent, Event, ExtensionUiRequest, Response, UiMethod, UiResponse,
};
