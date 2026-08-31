#![deny(clippy::unwrap_used, clippy::expect_used)]

pub mod domain;
pub mod error;
pub mod store;

pub use error::CfgStoreError;
