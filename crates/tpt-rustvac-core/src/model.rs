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
    /// Set by [`Arena::remove_subtree`]. A removed node stays in the arena so other ids are
    /// unchanged, but [`Arena::iter`] skips it.
    pub removed: bool,
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

    /// Every node that has not been removed.
    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter(|n| !n.removed)
    }

    /// Takes a node and everything below it out of the tree. Its size and file count come off
    /// every ancestor. A project left with no targets is removed too. Ids of other nodes do
    /// not change.
    pub fn remove_subtree(&mut self, id: NodeId) {
        let Some(node) = self.nodes.get(id.0) else {
            return;
        };
        if node.removed {
            return;
        }
        let (size, files, parent) = (node.size, node.files, node.parent);
        if let Some(p) = parent.and_then(|p| self.nodes.get_mut(p.0)) {
            p.children.retain(|c| *c != id);
        }
        let mut ancestor = parent;
        while let Some(a) = ancestor {
            let Some(node) = self.nodes.get_mut(a.0) else {
                break;
            };
            node.size = node.size.saturating_sub(size);
            node.files = node.files.saturating_sub(files);
            ancestor = node.parent;
        }

        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            if let Some(node) = self.nodes.get_mut(n.0) {
                node.removed = true;
                stack.extend(node.children.iter().copied());
            }
        }

        let emptied_project = parent
            .and_then(|p| self.nodes.get(p.0))
            .filter(|p| p.kind == Kind::Project && p.children.is_empty() && !p.removed)
            .map(|p| p.id);
        if let Some(project) = emptied_project {
            self.remove_subtree(project);
        }
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
            removed: false,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Root → project (with a target of 100 bytes and a source folder of 40) and a second
    /// project holding only a 7-byte target.
    fn sample() -> (Arena, NodeId, NodeId, NodeId, NodeId) {
        let mut arena = Arena::new();
        let root = arena.push(None, "root".into(), PathBuf::from("/r"), Kind::Root, false);
        let proj = arena.push(
            Some(root),
            "a".into(),
            PathBuf::from("/r/a"),
            Kind::Project,
            false,
        );
        let target = arena.push(
            Some(proj),
            "target".into(),
            PathBuf::from("/r/a/target"),
            Kind::Target,
            true,
        );
        let src = arena.push(
            Some(proj),
            "src".into(),
            PathBuf::from("/r/a/src"),
            Kind::Directory,
            false,
        );
        let other = arena.push(
            Some(root),
            "b".into(),
            PathBuf::from("/r/b"),
            Kind::Project,
            false,
        );
        let other_target = arena.push(
            Some(other),
            "target".into(),
            PathBuf::from("/r/b/target"),
            Kind::Target,
            true,
        );
        for (id, size, files) in [(target, 100, 3), (src, 40, 1), (other_target, 7, 1)] {
            let n = arena.get_mut(id).unwrap();
            n.size = size;
            n.files = files;
        }
        arena.get_mut(proj).unwrap().size = 140;
        arena.get_mut(proj).unwrap().files = 4;
        arena.get_mut(other).unwrap().size = 7;
        arena.get_mut(other).unwrap().files = 1;
        arena.get_mut(root).unwrap().size = 147;
        arena.get_mut(root).unwrap().files = 5;
        (arena, root, proj, target, other_target)
    }

    #[test]
    fn removing_a_target_takes_its_size_off_every_ancestor() {
        let (mut arena, root, proj, target, _) = sample();
        arena.remove_subtree(target);
        assert_eq!(arena.get(proj).unwrap().size, 40);
        assert_eq!(arena.get(proj).unwrap().files, 1);
        assert_eq!(arena.get(root).unwrap().size, 47);
        assert_eq!(arena.get(root).unwrap().files, 2);
        assert_eq!(arena.get(proj).unwrap().children.len(), 1);
    }

    #[test]
    fn removed_nodes_are_skipped_but_ids_stay_valid() {
        let (mut arena, _, _, target, other_target) = sample();
        let before = arena.len();
        arena.remove_subtree(target);
        assert_eq!(arena.len(), before);
        assert!(arena.iter().all(|n| n.id != target));
        assert!(arena.iter().any(|n| n.id == other_target));
        assert!(arena.get(other_target).is_some());
    }

    #[test]
    fn a_project_left_with_no_targets_goes_too() {
        let (mut arena, root, _, _, other_target) = sample();
        arena.remove_subtree(other_target);
        let b = arena.iter().find(|n| n.name == "b");
        assert!(b.is_none(), "emptied project should be removed");
        assert_eq!(arena.get(root).unwrap().size, 140);
        assert_eq!(arena.get(root).unwrap().children.len(), 1);
    }

    #[test]
    fn removing_twice_changes_nothing() {
        let (mut arena, root, _, target, _) = sample();
        arena.remove_subtree(target);
        arena.remove_subtree(target);
        assert_eq!(arena.get(root).unwrap().size, 47);
    }
}
