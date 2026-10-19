//! Shared filter rules used by the CLI, the GUI and compression.

use std::time::{Duration, SystemTime};

use crate::model::Node;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Only nodes whose newest file is at least this old.
    pub older_than: Option<Duration>,
    /// Only nodes at least this many bytes.
    pub min_size: Option<u64>,
}

impl Filter {
    /// A node with an unknown age never matches an age filter. That is the safe
    /// default, because the filter usually gates deletion.
    pub fn accepts(&self, node: &Node, now: SystemTime) -> bool {
        if let Some(min) = self.min_size {
            if node.size < min {
                return false;
            }
        }
        if let Some(older) = self.older_than {
            match node.newest {
                Some(t) => match now.duration_since(t) {
                    Ok(age) if age >= older => {}
                    _ => return false,
                },
                None => return false,
            }
        }
        true
    }
}
