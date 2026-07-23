//! Def-level structural diff: given two `Extraction`s of the same file at
//! different points in time, classify how each surviving def changed (or
//! didn't) using the sig/body fingerprints each `Language::extract`
//! implementation stamps onto every `ExtractedDef` (see
//! `fingerprint::split_fingerprint` / `fingerprint::module_init_fingerprint`).
//!
//! This module never parses anything itself — it operates purely on
//! `Extraction` values, so it has no dependency on tree-sitter or any
//! concrete language.

use std::collections::{HashMap, HashSet};

use crate::graph::DefKind;
use crate::language::{ExtractedDef, Extraction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefChange {
    /// A def present in `new` with no matching identity in `old`.
    Added { new_idx: usize },
    /// A def present in `old` with no matching identity in `new`.
    Removed { old_name: String, old_kind: DefKind },
    /// Signature hash unchanged, body hash changed.
    BodyChanged { new_idx: usize },
    /// Signature hash changed (regardless of whether body also changed).
    SigChanged { new_idx: usize },
    /// The file's `ModuleInit` def's top-level loose code changed.
    ModuleInitChanged,
}

/// Identity key a def is matched across extractions by: `TestCase`s match by
/// their full chain (`test_id`), everything else by `(kind, name)`. Modeled
/// as `(DefKind, Vec<String>)` so both cases share one map.
type Key = (DefKind, Vec<String>);

fn key_of(def: &ExtractedDef) -> Key {
    let ids = def
        .test_id
        .clone()
        .unwrap_or_else(|| vec![def.name.clone()]);
    (def.kind, ids)
}

/// Match old vs new extraction defs by identity key and compare hashes.
///
/// Matching: identity key = `(kind, test_id.clone().unwrap_or([name]))`.
/// Same-key collisions (overloads, same-named siblings) are paired in
/// declaration order; any unpaired remainder on the new side is `Added`, on
/// the old side is `Removed`. A pair with identical sig+body hashes yields no
/// entry.
pub fn diff_defs(old: &Extraction, new: &Extraction) -> Vec<DefChange> {
    let old_groups = group_by_key(&old.defs);
    let new_groups = group_by_key(&new.defs);

    let mut changes = Vec::new();
    for key in ordered_keys(&old.defs, &new.defs) {
        let empty: Vec<usize> = Vec::new();
        let olds = old_groups.get(&key).unwrap_or(&empty);
        let news = new_groups.get(&key).unwrap_or(&empty);
        let paired = olds.len().min(news.len());

        for i in 0..paired {
            let old_def = &old.defs[olds[i]];
            let new_idx = news[i];
            let new_def = &new.defs[new_idx];

            if new_def.kind == DefKind::ModuleInit {
                if old_def.sig_hash != new_def.sig_hash {
                    changes.push(DefChange::ModuleInitChanged);
                }
                continue;
            }

            if old_def.sig_hash != new_def.sig_hash {
                changes.push(DefChange::SigChanged { new_idx });
            } else if old_def.body_hash != new_def.body_hash {
                changes.push(DefChange::BodyChanged { new_idx });
            }
        }

        for &new_idx in &news[paired..] {
            changes.push(DefChange::Added { new_idx });
        }
        for &old_idx in &olds[paired..] {
            let d = &old.defs[old_idx];
            changes.push(DefChange::Removed {
                old_name: d.name.clone(),
                old_kind: d.kind,
            });
        }
    }
    changes
}

/// Group def indices by identity key, preserving declaration order within
/// each key's group (needed for order-paired collision matching).
fn group_by_key(defs: &[ExtractedDef]) -> HashMap<Key, Vec<usize>> {
    let mut groups: HashMap<Key, Vec<usize>> = HashMap::new();
    for (i, d) in defs.iter().enumerate() {
        groups.entry(key_of(d)).or_default().push(i);
    }
    groups
}

/// Deterministic key iteration order: first-seen in `new`, then any
/// old-only keys in their first-seen order in `old`.
fn ordered_keys(old_defs: &[ExtractedDef], new_defs: &[ExtractedDef]) -> Vec<Key> {
    let mut seen: HashSet<Key> = HashSet::new();
    let mut ordered = Vec::new();
    for d in new_defs {
        let k = key_of(d);
        if seen.insert(k.clone()) {
            ordered.push(k);
        }
    }
    for d in old_defs {
        let k = key_of(d);
        if seen.insert(k.clone()) {
            ordered.push(k);
        }
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::DefKind;

    fn def(name: &str, kind: DefKind, sig: u8, body: Option<u8>) -> ExtractedDef {
        ExtractedDef {
            name: name.into(),
            kind,
            start_line: 1,
            end_line: 2,
            test_id: None,
            computed_name: false,
            parent: None,
            sig_hash: [sig; 32],
            body_hash: body.map(|b| [b; 32]),
        }
    }

    fn ex(defs: Vec<ExtractedDef>) -> Extraction {
        Extraction {
            defs,
            imports: vec![],
            calls: vec![],
            reads: vec![],
        }
    }

    #[test]
    fn classifies_added_removed_body_sig_changes() {
        let old = ex(vec![
            def("<module>", DefKind::ModuleInit, 0, None),
            def("a", DefKind::Function, 1, Some(1)),
            def("b", DefKind::Function, 2, Some(2)),
            def("gone", DefKind::Function, 3, Some(3)),
        ]);
        let new = ex(vec![
            def("<module>", DefKind::ModuleInit, 0, None),
            def("a", DefKind::Function, 1, Some(9)), // body changed
            def("b", DefKind::Function, 9, Some(2)), // sig changed
            def("fresh", DefKind::Function, 4, Some(4)),
        ]);
        let changes = diff_defs(&old, &new);

        assert!(changes.iter().any(
            |c| matches!(c, DefChange::BodyChanged { new_idx } if new.defs[*new_idx].name == "a")
        ));
        assert!(changes.iter().any(
            |c| matches!(c, DefChange::SigChanged { new_idx } if new.defs[*new_idx].name == "b")
        ));
        assert!(changes.iter().any(
            |c| matches!(c, DefChange::Added { new_idx } if new.defs[*new_idx].name == "fresh")
        ));
        assert!(changes.iter().any(|c| matches!(
            c,
            DefChange::Removed { old_name, old_kind }
                if old_name == "gone" && *old_kind == DefKind::Function
        )));
        assert!(!changes
            .iter()
            .any(|c| matches!(c, DefChange::ModuleInitChanged)));
        // Exactly 4 changes: BodyChanged(a), SigChanged(b), Added(fresh),
        // Removed(gone) — the unchanged `<module>` yields no entry.
        assert_eq!(changes.len(), 4);
    }

    #[test]
    fn module_init_change_detected() {
        let old = ex(vec![def("<module>", DefKind::ModuleInit, 0, None)]);
        let new = ex(vec![def("<module>", DefKind::ModuleInit, 7, None)]);
        assert!(matches!(
            diff_defs(&old, &new)[..],
            [DefChange::ModuleInitChanged]
        ));
    }

    #[test]
    fn unchanged_defs_yield_no_entry() {
        let old = ex(vec![def("a", DefKind::Function, 1, Some(1))]);
        let new = ex(vec![def("a", DefKind::Function, 1, Some(1))]);
        assert!(diff_defs(&old, &new).is_empty());
    }

    #[test]
    fn collisions_pair_in_declaration_order() {
        // Two same-named/kind defs (e.g. overloads) on each side pair
        // positionally rather than cross-matching.
        let old = ex(vec![
            def("f", DefKind::Function, 1, Some(1)),
            def("f", DefKind::Function, 2, Some(2)),
        ]);
        let new = ex(vec![
            def("f", DefKind::Function, 1, Some(1)), // unchanged vs old[0]
            def("f", DefKind::Function, 9, Some(9)), // changed vs old[1]
        ]);
        let changes = diff_defs(&old, &new);
        assert_eq!(changes.len(), 1);
        assert!(matches!(changes[0], DefChange::SigChanged { new_idx: 1 }));
    }
}
