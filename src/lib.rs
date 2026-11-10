//! 8gent: one local endpoint in front of any model loader.
//!
//! - [`manifest`]: a loader is a TOML description (how to find, start and talk to an engine)
//! - [`library`]: the models on this computer, one entry per model with a build per format
//! - [`engine`]: starting, watching and stopping the engine that is loaded
//! - [`server`], [`proxy`]: the OpenAI-compatible endpoint and the control API
//! - [`normalize`]: making every loader's API behave the same for clients
//! - [`stats`]: requests answered, and speed per loader and model

pub mod config;
pub mod engine;
pub mod host;
pub mod import;
pub mod library;
pub mod manifest;
pub mod normalize;
pub mod paths;
pub mod proxy;
pub mod server;
pub mod service;
pub mod stats;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
