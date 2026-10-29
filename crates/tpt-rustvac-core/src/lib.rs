//! tpt-rustvac-core: scan, measure, compress and restore. Contains no UI code.
//!
//! The CLI and GUI both sit on top of this crate. Scanning runs on a background
//! thread and streams [`scan::ScanEvent`]s into an [`model::Arena`].

pub mod archive;
pub mod caches;
pub mod check;
pub mod config;
pub mod error;
pub mod filter;
pub mod identity;
pub mod model;
pub mod prune;
pub mod safety;
pub mod scan;
pub mod schedule;
pub mod sweep;
pub mod target;
pub mod toolchains;
pub mod walk;

pub use error::{Error, Result};
pub use model::{Arena, Kind, Node, NodeId, State};
pub use scan::{ScanEvent, ScanHandle, ScanMode, ScanRequest};
