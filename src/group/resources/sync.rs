//! Deterministic three-way synchronization. No timestamps decide which edit wins.
use super::safe;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const META: &str = ".desk-switch-sync";
pub type Manifest = BTreeMap<String, Node>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Node {
    Directory,
    File { size: u64, sha256: String },
}
impl Node {
    pub fn validate(&self) -> Result<()> {
        if let Self::File { size, sha256 } = self {
            ensure!(
                *size <= safe::MAX_TOTAL && safe::valid_hash(sha256),
                "Invalid sync fingerprint"
            );
        }
        Ok(())
    }
    pub fn size(&self) -> u64 {
        match self {
            Self::File { size, .. } => *size,
            Self::Directory => 0,
        }
    }
}

pub fn path(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 4096,
        "Invalid sync path"
    );
    let parts: Vec<_> = value.split('/').collect();
    ensure!(
        parts.len() <= 64 && !parts[0].eq_ignore_ascii_case(META),
        "Reserved sync path"
    );
    for part in parts {
        safe::valid_component(part)?;
    }
    Ok(())
}

pub fn validate(manifest: &Manifest) -> Result<()> {
    ensure!(manifest.len() <= safe::MAX_ENTRIES, "Too many sync entries");
    let mut normalized = BTreeSet::new();
    let mut total = 0_u64;
    for (name, node) in manifest {
        path(name)?;
        node.validate()?;
        ensure!(normalized.insert(name.to_lowercase()), "Sync paths collide");
        total = total
            .checked_add(node.size())
            .filter(|v| *v <= safe::MAX_TOTAL)
            .ok_or_else(|| anyhow::anyhow!("Sync folder exceeds size limit"))?;
        if let Some((parent, _)) = name.rsplit_once('/') {
            ensure!(
                manifest.get(parent) == Some(&Node::Directory),
                "Missing sync parent directory"
            );
        }
    }
    ensure!(
        serde_json::to_vec(manifest)?.len() <= safe::MAX_MANIFEST,
        "Sync manifest is too large"
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Side {
    Local,
    Remote,
}
impl Side {
    pub fn other(self) -> Self {
        match self {
            Self::Local => Self::Remote,
            Self::Remote => Self::Local,
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Resolution {
    Local,
    Remote,
    Skip,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Change {
    Set { value: Node },
    Delete,
    Rename { from: String, value: Node },
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Operation {
    pub side: Side,
    pub path: String,
    pub before: Option<Node>,
    pub change: Change,
}
impl Operation {
    pub fn validate(&self) -> Result<()> {
        path(&self.path)?;
        if let Some(node) = &self.before {
            node.validate()?;
        }
        match &self.change {
            Change::Set { value } => value.validate()?,
            Change::Rename { from, value } => {
                path(from)?;
                value.validate()?;
                ensure!(
                    from != &self.path
                        && self.before.is_none()
                        && matches!(value, Node::File { .. }),
                    "Invalid sync rename"
                );
            }
            Change::Delete => ensure!(self.before.is_some(), "Invalid sync deletion"),
        }
        Ok(())
    }
    pub fn after(&self) -> Option<&Node> {
        match &self.change {
            Change::Set { value } | Change::Rename { value, .. } => Some(value),
            Change::Delete => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Baseline {
    pub local: Manifest,
    pub remote: Manifest,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Plan {
    pub local: Manifest,
    pub remote: Manifest,
    pub operations: Vec<Operation>,
    pub conflicts: Vec<String>,
    pub skipped: Vec<String>,
}
impl Plan {
    pub fn id(&self) -> Result<String> {
        Ok(safe::hash(&serde_json::to_vec(self)?))
    }
    pub fn validate(&self) -> Result<()> {
        validate(&self.local)?;
        validate(&self.remote)?;
        ensure!(
            self.operations.len() <= safe::MAX_ENTRIES * 2 && self.conflicts.is_empty(),
            "Resolve sync conflicts first"
        );
        let mut destinations = BTreeSet::new();
        for op in &self.operations {
            op.validate()?;
            ensure!(
                destinations.insert((matches!(op.side, Side::Local), op.path.to_lowercase())),
                "Duplicate sync destination"
            );
        }
        Ok(())
    }
    pub fn bytes(&self) -> u64 {
        self.operations
            .iter()
            .filter_map(|op| match &op.change {
                Change::Set { value } => Some(value.size()),
                _ => None,
            })
            .sum()
    }
    pub fn expected(&self) -> Baseline {
        let mut result = Baseline {
            local: self.local.clone(),
            remote: self.remote.clone(),
        };
        for op in &self.operations {
            let manifest = if op.side == Side::Local {
                &mut result.local
            } else {
                &mut result.remote
            };
            if let Change::Rename { from, .. } = &op.change {
                manifest.remove(from);
            }
            if let Some(value) = op.after() {
                manifest.insert(op.path.clone(), value.clone());
            } else {
                manifest.remove(&op.path);
            }
        }
        result
    }
}

pub fn compare(
    local: Manifest,
    remote: Manifest,
    baseline: Option<&Baseline>,
    choices: &BTreeMap<String, Resolution>,
) -> Result<Plan> {
    validate(&local)?;
    validate(&remote)?;
    if let Some(base) = baseline {
        validate(&base.local)?;
        validate(&base.remote)?;
    }
    let keys: BTreeSet<_> = local
        .keys()
        .chain(remote.keys())
        .chain(
            baseline
                .into_iter()
                .flat_map(|b| b.local.keys().chain(b.remote.keys())),
        )
        .cloned()
        .collect();
    // A case-only collision or file/directory substitution needs a local rename.
    let mut names = BTreeMap::new();
    for name in &keys {
        if let Some(previous) = names.insert(name.to_lowercase(), name) {
            ensure!(previous == name, "Sync paths collide across computers");
        }
        if let (Some(a), Some(b)) = (local.get(name), remote.get(name)) {
            ensure!(
                matches!(a, Node::Directory) == matches!(b, Node::Directory),
                "A file and directory have the same sync path; rename one first"
            );
        }
    }
    let mut plan = Plan {
        local: local.clone(),
        remote: remote.clone(),
        operations: Vec::new(),
        conflicts: Vec::new(),
        skipped: Vec::new(),
    };
    for name in keys {
        let a = local.get(&name);
        let b = remote.get(&name);
        if a == b {
            continue;
        }
        let winner = if let Some(base) = baseline {
            match (a != base.local.get(&name), b != base.remote.get(&name)) {
                (true, false) => Some(Side::Local),
                (false, true) => Some(Side::Remote),
                _ => None,
            }
        } else {
            match (a, b) {
                (Some(_), None) => Some(Side::Local),
                (None, Some(_)) => Some(Side::Remote),
                _ => None,
            }
        };
        let winner = winner.or_else(|| match choices.get(&name) {
            Some(Resolution::Local) => Some(Side::Local),
            Some(Resolution::Remote) => Some(Side::Remote),
            _ => None,
        });
        let Some(winner) = winner else {
            if choices.get(&name) == Some(&Resolution::Skip) {
                plan.skipped.push(name);
            } else {
                plan.conflicts.push(name);
            }
            continue;
        };
        let (source, target) = if winner == Side::Local {
            (a, b)
        } else {
            (b, a)
        };
        plan.operations.push(Operation {
            side: winner.other(),
            path: name,
            before: target.cloned(),
            change: source.map_or(Change::Delete, |node| Change::Set {
                value: node.clone(),
            }),
        });
    }
    // If a parent deletion has a retained child, retaining that directory is the
    // only safe operation. In particular, Skip never causes a recursive delete.
    let deleted = |side, name: &str, operations: &[Operation]| {
        operations
            .iter()
            .any(|op| op.side == side && op.path == name && op.change == Change::Delete)
    };
    plan.operations = plan
        .operations
        .iter()
        .filter(|op| {
            if op.change != Change::Delete || op.before != Some(Node::Directory) {
                return true;
            }
            let target = if op.side == Side::Local {
                &local
            } else {
                &remote
            };
            !target.keys().any(|name| {
                name.starts_with(&format!("{}/", op.path))
                    && !deleted(op.side, name, &plan.operations)
            })
        })
        .cloned()
        .collect();
    collapse_renames(&mut plan.operations);
    plan.operations.sort_by_key(|op| {
        let depth = op.path.split('/').count();
        let category = match op.change {
            Change::Set {
                value: Node::Directory,
            } => 0,
            Change::Delete => 2,
            _ => 1,
        };
        (
            category,
            if category == 2 { 65 - depth } else { depth },
            op.path.clone(),
        )
    });
    Ok(plan)
}

fn collapse_renames(operations: &mut Vec<Operation>) {
    let mut remove = BTreeSet::new();
    for index in 0..operations.len() {
        let op = &operations[index];
        let Change::Set {
            value: value @ Node::File { .. },
        } = &op.change
        else {
            continue;
        };
        if op.before.is_some() {
            continue;
        }
        let matches: Vec<_> = operations
            .iter()
            .enumerate()
            .filter(|(_, old)| {
                old.side == op.side
                    && old.change == Change::Delete
                    && old.before.as_ref() == Some(value)
            })
            .map(|(i, _)| i)
            .collect();
        let count = operations
            .iter()
            .filter(|v| v.side == op.side && v.before.is_none() && v.change == op.change)
            .count();
        if matches.len() == 1 && count == 1 && !remove.contains(&matches[0]) {
            let old = matches[0];
            operations[index].change = Change::Rename {
                from: operations[old].path.clone(),
                value: value.clone(),
            };
            remove.insert(old);
        }
    }
    *operations = operations
        .drain(..)
        .enumerate()
        .filter_map(|(i, op)| (!remove.contains(&i)).then_some(op))
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    fn files(values: &[(&str, &str)]) -> Manifest {
        values
            .iter()
            .map(|(path, content)| {
                (
                    (*path).into(),
                    Node::File {
                        size: content.len() as u64,
                        sha256: safe::hash(content.as_bytes()),
                    },
                )
            })
            .collect()
    }
    #[test]
    fn first_comparison_unions_without_deleting_and_requires_conflict_choice() {
        let a = files(&[("a", "a"), ("both", "left")]);
        let b = files(&[("b", "b"), ("both", "right")]);
        let plan = compare(a.clone(), b.clone(), None, &BTreeMap::new()).unwrap();
        assert_eq!(plan.conflicts, ["both"]);
        assert_eq!(plan.operations.len(), 2);
        assert!(!plan.operations.iter().any(|v| v.change == Change::Delete));
        let plan = compare(
            a,
            b,
            None,
            &BTreeMap::from([("both".into(), Resolution::Local)]),
        )
        .unwrap();
        plan.validate().unwrap();
        assert_eq!(plan.expected().local, plan.expected().remote);
    }
    #[test]
    fn three_way_edits_deletes_rename_and_skip_are_distinct() {
        let old = files(&[
            ("rename", "same"),
            ("edit", "old"),
            ("delete", "old"),
            ("conflict", "old"),
        ]);
        let base = Baseline {
            local: old.clone(),
            remote: old.clone(),
        };
        let a = files(&[("renamed", "same"), ("edit", "new"), ("conflict", "left")]);
        let b = files(&[
            ("rename", "same"),
            ("edit", "old"),
            ("delete", "old"),
            ("conflict", "right"),
        ]);
        let plan = compare(
            a,
            b,
            Some(&base),
            &BTreeMap::from([("conflict".into(), Resolution::Skip)]),
        )
        .unwrap();
        assert_eq!(plan.skipped, ["conflict"]);
        assert!(
            plan.operations
                .iter()
                .any(|v| matches!(v.change, Change::Rename { .. }))
        );
        assert!(
            plan.operations
                .iter()
                .any(|v| v.path == "delete" && v.change == Change::Delete)
        );
        assert!(plan.conflicts.is_empty());
    }
    #[test]
    fn delete_against_edit_conflicts_and_portable_paths_fail_closed() {
        let old = files(&[("a", "old")]);
        let plan = compare(
            Manifest::new(),
            files(&[("a", "new")]),
            Some(&Baseline {
                local: old.clone(),
                remote: old,
            }),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(plan.conflicts, ["a"]);
        for invalid in [
            "../a",
            "a/../b",
            "C:/x",
            "a\\b",
            "/etc",
            "CON",
            ".desk-switch-sync/x",
            "a/",
            "a. ",
        ] {
            assert!(path(invalid).is_err(), "{invalid}");
        }
        assert!(
            compare(
                files(&[("a", "a")]),
                files(&[("A", "a")]),
                None,
                &BTreeMap::new()
            )
            .is_err()
        );
    }
}
