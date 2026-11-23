//! `tag-control`: TAG's local control plane.
//!
//! One protocol (newline-delimited JSON-RPC 2.0 over a private Unix socket), three front-ends:
//! the socket itself, `tag ctl`, and `tag mcp`. See `docs/orchestration-roadmap.md` §4.
//!
//! - [`methods`]: every method's name, tier, params and result types, plus [`methods::CATALOG`].
//! - [`protocol`]: JSON-RPC envelopes and framing.
//! - [`client`]: blocking and async clients.
//! - [`discovery`]: finding the socket.

pub mod client;
pub mod discovery;
pub mod methods;
pub mod protocol;
pub mod server;

pub use client::{AsyncControlClient, ClientError, ControlClient};
pub use methods::{method_info, Method, MethodInfo, CATALOG, PROTOCOL_VERSION};
pub use server::{serve, ServerHandle, ServerSettings, TokenGrant, TokenRegistry};
pub use tag_core::control::{
    codes, ClientCtx, ControlEnvelope, ControlError, ControlResult, ReplyHandle, Role, Tier,
};
pub use tag_core::event::StateEvent;
