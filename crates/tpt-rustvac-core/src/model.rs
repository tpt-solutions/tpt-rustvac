//! Flat arena of nodes. Nodes refer to each other by index, so consumers can
//! mutate `expanded` / `selected` while iterating without fighting the borrow
//! checker.

use std::path::PathBuf;
use std::time::SystemTime;

/// Index of a node inside an [`Arena`]. Index 0 is always the scan root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Root,
    /// Only produced in Rust-target mode.
    Project,
    /// Only produced in Rust-target mode.
    Target,
    /// Only produced in general mode.
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Pending,
    Scanning,
    Done,
    Failed(String),
    /// Something inside was modified within the busy grace period.
    Busy,
    /// A matching archive exists in the vacuum.
    Archived,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
    /// Bytes on disk of unique files below this node (hardlinks counted once). See
    /// `identity::disk_bytes`.
    pub size: u64,
    /// Count of unique files below this node.
    pub files: u64,
    /// Newest mtime below this node. `None` means unknown and never blocks.
    pub newest: Option<SystemTime>,
    pub state: State,
    pub expanded: bool,
    pub selected: bool,
    pub children: Vec<NodeId>,
    pub is_target: bool,
}

#[derive(Debug, Default, Clone)]
pub struct Arena {
    nodes: Vec<Node>,
}

impl Arena {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn root(&self) -> Option<&Node> {
        self.nodes.first()
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.0)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id.0)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter()
    }

    /// Appends a node and links it under `parent`. The returned id equals the
    /// node's index, so ids are stable for the life of the arena.
    pub fn push(
        &mut self,
        parent: Option<NodeId>,
        name: String,
        path: PathBuf,
        kind: Kind,
        is_target: bool,
    ) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node {
            id,
            parent,
            name,
            path,
            kind,
            size: 0,
            files: 0,
            newest: None,
            state: State::Pending,
            expanded: parent.is_none(),
            selected: false,
            children: Vec::new(),
            is_target,
        });
        if let Some(p) = parent {
            if let Some(parent_node) = self.nodes.get_mut(p.0) {
                parent_node.children.push(id);
            }
        }
        id
    }

    /// Sum of `size` over the given ids, skipping ids that are not in the arena.
    pub fn total_size<'a>(&self, ids: impl IntoIterator<Item = &'a NodeId>) -> u64 {
        ids.into_iter()
            .filter_map(|id| self.get(*id))
            .map(|n| n.size)
            .sum()
    }
}
