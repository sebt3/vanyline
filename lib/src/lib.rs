#![deny(clippy::unwrap_used, clippy::expect_used)]

use percent_encoding::SIMPLE_ENCODE_SET;
use percent_encoding::define_encode_set;
use percent_encoding::utf8_percent_encode;

define_encode_set! {
    pub GIT_ENCODE_SET = [SIMPLE_ENCODE_SET] | {' ', '%'}
}

/// Encodage percent d'un chemin git, segment par segment, en PRÉSERVANT les
/// séparateurs `/` et les caractères sûrs `-`, `_`, `.`, `~`.
pub fn encode_git_path(path: &str) -> String {
    utf8_percent_encode(path, GIT_ENCODE_SET).to_string()
}

pub mod builtin;
pub use vanyline_cfgstore::{domain, store};
mod error;
pub mod event;
#[cfg(feature = "k8s")]
pub mod k8s;
pub mod model;
pub mod prefixed_mcp;
pub mod session;
mod types;

pub use error::*;
pub use prefixed_mcp::*;
pub use types::*;

pub use rig_core::tool::server::ToolServerHandle;
