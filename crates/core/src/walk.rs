//! The impact walker: a reverse-reachability BFS from a set of seed defs
//! out to every `TestCase` def that could observe a change to those seeds.
//! See `.superpowers/sdd/task-1-brief.md` for the full algorithm this
//! implements.
//!
//! Reverse edges walked (a def D's *reverse* reachability):
//! - `Calls{from, to: Resolved(D)}` -> `from` is impacted (D's caller).
//! - `Reads{from, to: D}` -> `from` is impacted (D's reader).
//! - `Contains{parent, child: D}` -> `parent` is impacted (D's container:
//!   the parent's behavior embeds D, e.g. a class containing a method):
//!   *unless* `parent` is a `ModuleInit`: the indexer wires every parentless
//!   def (an ordinary top-level function, not just genuinely nested ones)
//!   to its file's `ModuleInit` as a bookkeeping fallback, and that edge
//!   doesn't represent real behavioral embedding, so it's excluded here to
//!   avoid spuriously widening every top-level def change to the whole
//!   file's (and its transitive importers') tests.
//! - `Calls{from, to: Unknown(name)}` where `name == short_name(D)` ->
//!   `from` is impacted (an unresolved call that could dynamically dispatch
//!   to D).
//! - If D is a `ModuleInit`: every file in the transitive importer closure
//!   of D's file (including D's own file) has its `ModuleInit` enqueued and
//!   its `TestCase` defs enqueued too (not just collected; a widened test
//!   must keep propagating through its own callers just like any other
//!   visited `TestCase`; importing a module re-runs that module's top-level
//!   side effects, and running any test in an importing file re-runs the
//!   whole import chain).
//!
//! `TestCase` defs are collected when visited, but the walk still continues
//! through them: a test helper (itself a `TestCase`) that's called by other
//! tests must keep propagating impact to those callers.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::classify::Seed;
use crate::graph::{CallTarget, DefId, DefKind, Edge, FileId, Graph};

/// All `TestCase` defs impacted by `seeds`, per the spec's reverse-
/// reachability rules. Deterministic order (ascending `DefId`).
///
/// `Seed.kind` is intentionally not consulted here: in v1, `Body`,
/// `Signature`, `Added`, and `ModuleInit` seeds all walk identically,
/// because the reverse edges built below already reach every referencer of
/// a def regardless of *what* changed about it. `kind` stays on `Seed` for
/// future precision (e.g. a pure-signature-compatible rename could
/// eventually skip re-walking callers) and for richer reporting; it's
/// reserved, not dead weight.
pub fn impacted_tests(graph: &Graph, seeds: &[Seed]) -> Vec<DefId> {
    let index = ReverseIndex::build(graph);

    let mut visited: HashSet<DefId> = HashSet::new();
    let mut queue: VecDeque<DefId> = VecDeque::new();
    let mut tests: HashSet<DefId> = HashSet::new();

    for seed in seeds {
        enqueue(&mut queue, &mut visited, seed.def);
    }

    while let Some(d) = queue.pop_front() {
        let def = graph.def(d);

        if def.kind == DefKind::TestCase {
            tests.insert(d);
            // Do NOT stop here: a test helper called by other tests must
            // keep propagating impact to its own callers.
        }

        if let Some(callers) = index.callers.get(&d) {
            for &c in callers {
                enqueue(&mut queue, &mut visited, c);
            }
        }
        if let Some(readers) = index.readers.get(&d) {
            for &r in readers {
                enqueue(&mut queue, &mut visited, r);
            }
        }
        if let Some(&parent) = index.containers.get(&d) {
            // `indexer::index_repo_incremental` wires every parentless def
            // (an ordinary top-level function, not just genuinely nested
            // ones) to its file's `ModuleInit` via `Contains`, purely as
            // bookkeeping; there is no class/struct whose behavior
            // "embeds" a plain top-level function merely because they share
            // a file. Reverse-propagating through THAT edge would make the
            // `ModuleInit` widening below (importer-closure + all of the
            // file's tests) fire for literally any top-level def change,
            // defeating precise selection for the common case. Only widen
            // through a `Contains` edge when the parent is genuine
            // behavioral containment (e.g. a class containing a method); a
            // real module-init change still widens correctly below, via
            // its own `SeedKind::ModuleInit` seed or via the importer-
            // closure loop enqueuing `ModuleInit`s directly (never through
            // this `containers` edge).
            if graph.def(parent).kind != DefKind::ModuleInit {
                enqueue(&mut queue, &mut visited, parent);
            }
        }
        if let Some(unknown_callers) = index.unknown_by_name.get(short_name(&def.name)) {
            for &c in unknown_callers {
                enqueue(&mut queue, &mut visited, c);
            }
        }

        if def.kind == DefKind::ModuleInit {
            for file in index.importer_closure(def.file) {
                if let Some(m) = graph.module_init(file) {
                    enqueue(&mut queue, &mut visited, m);
                }
                for (id, file_def) in graph.defs_in_file(file) {
                    if file_def.kind == DefKind::TestCase {
                        // Enqueue (not just collect) so the main loop's
                        // uniform TestCase handling both records the test
                        // AND keeps propagating through its own callers:
                        // otherwise a test helper collected here would
                        // silently stop the walk short of any external
                        // caller of that helper.
                        enqueue(&mut queue, &mut visited, id);
                    }
                }
            }
        }
    }

    let mut result: Vec<DefId> = tests.into_iter().collect();
    result.sort();
    result
}

fn enqueue(queue: &mut VecDeque<DefId>, visited: &mut HashSet<DefId>, d: DefId) {
    if visited.insert(d) {
        queue.push_back(d);
    }
}

/// The last `.`-separated segment of a def's name (methods are recorded as
/// e.g. `Class.method`; a plain function's short name is its whole name).
/// Used to widen the walk through `Calls{to: Unknown(name)}` edges, since an
/// unresolved call site only ever recorded the bare name it referenced.
fn short_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// One-time reverse indexes over a `Graph`'s edges, so the BFS in
/// `impacted_tests` never has to scan `graph.edges` more than once.
struct ReverseIndex<'g> {
    /// `Calls{from, to: Resolved(t)}` -> `callers[t] = [from, ...]`.
    callers: HashMap<DefId, Vec<DefId>>,
    /// `Reads{from, to}` -> `readers[to] = [from, ...]`.
    readers: HashMap<DefId, Vec<DefId>>,
    /// `Contains{parent, child}` -> `containers[child] = parent`.
    containers: HashMap<DefId, DefId>,
    /// `Calls{from, to: Unknown(name)}` -> `unknown_by_name[name] = [from, ...]`.
    unknown_by_name: HashMap<&'g str, Vec<DefId>>,
    /// `Imports{from, to}` -> `importers[to] = [from, ...]` (files that
    /// import `to`, i.e. `to`'s importers).
    importers: HashMap<FileId, Vec<FileId>>,
}

impl<'g> ReverseIndex<'g> {
    fn build(graph: &'g Graph) -> Self {
        let mut callers: HashMap<DefId, Vec<DefId>> = HashMap::new();
        let mut readers: HashMap<DefId, Vec<DefId>> = HashMap::new();
        let mut containers: HashMap<DefId, DefId> = HashMap::new();
        let mut unknown_by_name: HashMap<&str, Vec<DefId>> = HashMap::new();
        let mut importers: HashMap<FileId, Vec<FileId>> = HashMap::new();

        for edge in &graph.edges {
            match edge {
                Edge::Calls {
                    from,
                    to: CallTarget::Resolved(t),
                } => callers.entry(*t).or_default().push(*from),
                Edge::Calls {
                    from,
                    to: CallTarget::Unknown(name),
                } => unknown_by_name
                    .entry(name.as_str())
                    .or_default()
                    .push(*from),
                Edge::Reads { from, to } => readers.entry(*to).or_default().push(*from),
                Edge::Contains { parent, child } => {
                    containers.insert(*child, *parent);
                }
                Edge::Imports { from, to } => importers.entry(*to).or_default().push(*from),
            }
        }

        ReverseIndex {
            callers,
            readers,
            containers,
            unknown_by_name,
            importers,
        }
    }

    /// The transitive closure of files that (directly or indirectly) import
    /// `file`, including `file` itself; importing a module re-runs it, so
    /// this is exactly the set of files whose `ModuleInit` needs
    /// re-enqueuing and whose tests re-run the change.
    fn importer_closure(&self, file: FileId) -> Vec<FileId> {
        let mut visited: HashSet<FileId> = HashSet::new();
        let mut queue: VecDeque<FileId> = VecDeque::new();
        let mut out = Vec::new();

        visited.insert(file);
        queue.push_back(file);
        while let Some(f) = queue.pop_front() {
            out.push(f);
            if let Some(imps) = self.importers.get(&f) {
                for &imp in imps {
                    if visited.insert(imp) {
                        queue.push_back(imp);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::SeedKind;
    use crate::graph::FileNode;
    use std::path::PathBuf;

    /// A fresh, empty graph to build a hand-authored scenario on top of.
    fn g() -> Graph {
        Graph::default()
    }

    fn file(g: &mut Graph, path: &str) -> FileId {
        g.add_file(FileNode {
            path: PathBuf::from(path),
            hash: [0; 32],
            lang: "ts".into(),
        })
    }

    fn def(g: &mut Graph, name: &str, kind: DefKind, file: FileId) -> DefId {
        g.add_def(crate::graph::Def {
            name: name.into(),
            kind,
            file,
            start_line: 1,
            end_line: 2,
            test_id: None,
            computed_name: false,
        })
    }

    fn seed(def: DefId) -> Seed {
        Seed {
            def,
            kind: SeedKind::Body,
        }
    }

    #[test]
    fn caller_chain_reaches_test() {
        // add <- calculate <- test "t1"; seed add(Body) -> [t1]
        let mut g = g();
        let f = file(&mut g, "a.ts");
        let add = def(&mut g, "add", DefKind::Function, f);
        let calculate = def(&mut g, "calculate", DefKind::Function, f);
        let t1 = def(&mut g, "t1", DefKind::TestCase, f);
        g.add_edge(Edge::Calls {
            from: calculate,
            to: CallTarget::Resolved(add),
        });
        g.add_edge(Edge::Calls {
            from: t1,
            to: CallTarget::Resolved(calculate),
        });

        let result = impacted_tests(&g, &[seed(add)]);
        assert_eq!(result, vec![t1]);
    }

    #[test]
    fn unknown_name_widens_to_matching_callers() {
        // test t2 has Calls{Unknown("add")}; seed def add -> t2 selected
        let mut g = g();
        let f = file(&mut g, "a.ts");
        let add = def(&mut g, "add", DefKind::Function, f);
        let t2 = def(&mut g, "t2", DefKind::TestCase, f);
        g.add_edge(Edge::Calls {
            from: t2,
            to: CallTarget::Unknown("add".into()),
        });

        let result = impacted_tests(&g, &[seed(add)]);
        assert_eq!(result, vec![t2]);
    }

    #[test]
    fn module_init_seed_selects_transitive_importer_tests() {
        // file A (module_init M) <- imports - file B <- imports - file C(test t3)
        // seed M -> t3 selected (via importer closure), tests in A too
        let mut g = g();
        let fa = file(&mut g, "a.ts");
        let fb = file(&mut g, "b.ts");
        let fc = file(&mut g, "c.ts");

        let m_a = def(&mut g, "<module>", DefKind::ModuleInit, fa);
        let t_a = def(&mut g, "ta", DefKind::TestCase, fa);
        let _m_b = def(&mut g, "<module>", DefKind::ModuleInit, fb);
        let _m_c = def(&mut g, "<module>", DefKind::ModuleInit, fc);
        let t3 = def(&mut g, "t3", DefKind::TestCase, fc);

        // B imports A; C imports B.
        g.add_edge(Edge::Imports { from: fb, to: fa });
        g.add_edge(Edge::Imports { from: fc, to: fb });

        let result = impacted_tests(&g, &[seed(m_a)]);
        let mut expected = vec![t_a, t3];
        expected.sort();
        assert_eq!(result, expected);
    }

    #[test]
    fn reads_edge_reaches_test() {
        // test reads CONST; seed CONST def -> test
        let mut g = g();
        let f = file(&mut g, "a.ts");
        let constant = def(&mut g, "CONST", DefKind::Function, f);
        let t4 = def(&mut g, "t4", DefKind::TestCase, f);
        g.add_edge(Edge::Reads {
            from: t4,
            to: constant,
        });

        let result = impacted_tests(&g, &[seed(constant)]);
        assert_eq!(result, vec![t4]);
    }

    #[test]
    fn contains_reverse_reaches_parent() {
        // class C contains method m; test calls... m's parent C: seed m -> C enqueued;
        // test calling C (constructor) -> selected
        let mut g = g();
        let f = file(&mut g, "a.ts");
        let class_c = def(&mut g, "C", DefKind::Class, f);
        let method_m = def(&mut g, "C.m", DefKind::Method, f);
        let t5 = def(&mut g, "t5", DefKind::TestCase, f);
        g.add_edge(Edge::Contains {
            parent: class_c,
            child: method_m,
        });
        g.add_edge(Edge::Calls {
            from: t5,
            to: CallTarget::Resolved(class_c),
        });

        let result = impacted_tests(&g, &[seed(method_m)]);
        assert_eq!(result, vec![t5]);
    }

    /// Regression: `index_repo_incremental` wires every parentless def (an
    /// ordinary top-level function, not just genuinely nested ones) to its
    /// file's `ModuleInit` via `Contains`, purely as bookkeeping; that
    /// edge must NOT reverse-propagate into the `ModuleInit` importer-
    /// closure widening, or every top-level def change would sweep in
    /// every test in its file (and every transitively importing file)
    /// regardless of whether it actually calls/reads the changed def.
    /// Here `fn_add` and `fn_other` are unrelated siblings in the same
    /// file, each only its own file's `Contains`-child of `ModuleInit`
    /// (mirroring real indexer output, NOT a genuine module-init change);
    /// seeding `fn_add` must select only `test_add` (a real caller), never
    /// `test_other` (which calls the unrelated sibling).
    #[test]
    fn module_init_container_does_not_widen_unrelated_top_level_def() {
        let mut g = g();
        let f = file(&mut g, "a.ts");
        let m_a = def(&mut g, "<module>", DefKind::ModuleInit, f);
        let fn_add = def(&mut g, "add", DefKind::Function, f);
        let fn_other = def(&mut g, "other", DefKind::Function, f);
        let test_add = def(&mut g, "test_add", DefKind::TestCase, f);
        let test_other = def(&mut g, "test_other", DefKind::TestCase, f);

        g.add_edge(Edge::Contains {
            parent: m_a,
            child: fn_add,
        });
        g.add_edge(Edge::Contains {
            parent: m_a,
            child: fn_other,
        });
        g.add_edge(Edge::Contains {
            parent: m_a,
            child: test_add,
        });
        g.add_edge(Edge::Contains {
            parent: m_a,
            child: test_other,
        });
        g.add_edge(Edge::Calls {
            from: test_add,
            to: CallTarget::Resolved(fn_add),
        });
        g.add_edge(Edge::Calls {
            from: test_other,
            to: CallTarget::Resolved(fn_other),
        });

        let result = impacted_tests(&g, &[seed(fn_add)]);
        assert_eq!(result, vec![test_add]);
    }

    #[test]
    fn test_helper_propagation() {
        // helper TestCase h called by test t; seed something reaching h -> both h and t collected
        let mut g = g();
        let f = file(&mut g, "a.ts");
        let src = def(&mut g, "src", DefKind::Function, f);
        let helper = def(&mut g, "h", DefKind::TestCase, f);
        let test_t = def(&mut g, "t", DefKind::TestCase, f);
        g.add_edge(Edge::Calls {
            from: helper,
            to: CallTarget::Resolved(src),
        });
        g.add_edge(Edge::Calls {
            from: test_t,
            to: CallTarget::Resolved(helper),
        });

        let result = impacted_tests(&g, &[seed(src)]);
        let mut expected = vec![helper, test_t];
        expected.sort();
        assert_eq!(result, expected);
    }

    #[test]
    fn module_init_widened_test_propagates_to_external_caller() {
        // File A: module_init M_a, helper TestCase H (no import relationship
        // needed here: H is collected directly as a TestCase in A's own
        // file during the module_init importer-closure step). File B:
        // TestCase T_ext calls H directly (Calls{Resolved(H)}).
        // Seed M_a -> H is collected via the module_init closure step, but
        // that collection must ALSO enqueue H (not just record it as a
        // test) so the main loop keeps walking through H's callers and
        // reaches T_ext, per "test helper propagation": a test helper
        // reached via ANY path (including module-init widening, not just
        // ordinary caller chains) must keep propagating to its own callers.
        let mut g = g();
        let fa = file(&mut g, "a.ts");
        let fb = file(&mut g, "b.ts");

        let m_a = def(&mut g, "<module>", DefKind::ModuleInit, fa);
        let helper_h = def(&mut g, "h", DefKind::TestCase, fa);
        let t_ext = def(&mut g, "t_ext", DefKind::TestCase, fb);
        g.add_edge(Edge::Calls {
            from: t_ext,
            to: CallTarget::Resolved(helper_h),
        });

        let result = impacted_tests(&g, &[seed(m_a)]);
        let mut expected = vec![helper_h, t_ext];
        expected.sort();
        assert_eq!(result, expected);
    }

    #[test]
    fn unknown_widening_fires_for_def_reached_mid_walk() {
        // Seed X; Y calls X (Calls{from: Y, to: Resolved(X)}), so visiting
        // (seeded) X enqueues Y as its caller; Y is reached MID-WALK, not
        // itself a seed. Elsewhere, test T has an unresolved call matching
        // Y's short name. Widening must be evaluated for every def dequeued
        // during the walk (not just seeds), so T must still be selected.
        let mut g = g();
        let f = file(&mut g, "a.ts");
        let x = def(&mut g, "x", DefKind::Function, f);
        let y = def(&mut g, "y", DefKind::Function, f);
        let t = def(&mut g, "t", DefKind::TestCase, f);
        g.add_edge(Edge::Calls {
            from: y,
            to: CallTarget::Resolved(x),
        });
        g.add_edge(Edge::Calls {
            from: t,
            to: CallTarget::Unknown("y".into()),
        });

        let result = impacted_tests(&g, &[seed(x)]);
        assert_eq!(result, vec![t]);
    }

    #[test]
    fn dotted_method_name_widens_via_unknown_call() {
        // Visited def "Calc.push" (a Method, seeded directly); test with
        // Calls{Unknown("push")} must match on the last `.`-segment of the
        // dotted method name.
        let mut g = g();
        let f = file(&mut g, "a.ts");
        let push_method = def(&mut g, "Calc.push", DefKind::Method, f);
        let t = def(&mut g, "t", DefKind::TestCase, f);
        g.add_edge(Edge::Calls {
            from: t,
            to: CallTarget::Unknown("push".into()),
        });

        let result = impacted_tests(&g, &[seed(push_method)]);
        assert_eq!(result, vec![t]);
    }

    #[test]
    fn deterministic_order_and_no_dupes() {
        // two seeds reaching same test -> once, sorted
        let mut g = g();
        let f = file(&mut g, "a.ts");
        let a = def(&mut g, "a", DefKind::Function, f);
        let b = def(&mut g, "b", DefKind::Function, f);
        let t_c = def(&mut g, "tc", DefKind::TestCase, f);
        g.add_edge(Edge::Calls {
            from: t_c,
            to: CallTarget::Resolved(a),
        });
        g.add_edge(Edge::Calls {
            from: t_c,
            to: CallTarget::Resolved(b),
        });
        // Two more tests, seeded directly (as TestCase seeds), out of
        // ascending order, to check the output is sorted regardless of
        // seed order or discovery order.
        let t_a = def(&mut g, "ta", DefKind::TestCase, f);
        let t_b = def(&mut g, "tb", DefKind::TestCase, f);

        let seeds = vec![seed(b), seed(t_b), seed(t_a), seed(a)];
        let result = impacted_tests(&g, &seeds);

        assert_eq!(result, vec![t_c, t_a, t_b].into_iter().collect::<Vec<_>>());
        // Explicitly assert ascending order + no duplicates.
        let mut sorted = result.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(result, sorted);
    }
}
