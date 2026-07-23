//! Structural fingerprints: comment/whitespace-insensitive hashing of
//! tree-sitter subtrees. Two subtrees fingerprint equal iff they are
//! token-identical modulo comments and formatting.

use tree_sitter::Node;

/// Hash of a subtree's STRUCTURE + token text, skipping comments and
/// whitespace: two subtrees fingerprint equal iff they are token-identical
/// modulo comments/formatting.
pub fn fingerprint(node: Node, src: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hash_walk(node, src, &mut hasher, None);
    *hasher.finalize().as_bytes()
}

/// Fingerprint of `node` excluding its `body` field child (the "signature"),
/// and of the body alone. body=None when the node has no body field.
pub fn split_fingerprint(node: Node, src: &[u8]) -> (/*sig*/ [u8; 32], /*body*/ Option<[u8; 32]>) {
    let body = node.child_by_field_name("body");
    let skip_id = body.map(|b| b.id());

    let mut hasher = blake3::Hasher::new();
    hash_walk(node, src, &mut hasher, skip_id);
    let sig = *hasher.finalize().as_bytes();

    let body_fp = body.map(|b| fingerprint(b, src));
    (sig, body_fp)
}

/// Fingerprint over `root`'s direct children, excluding comments and
/// excluding any child for which `skip` returns true (each language passes a
/// closure marking its def/import node kinds).
pub fn module_init_fingerprint(
    root: Node,
    src: &[u8],
    skip: &dyn Fn(&tree_sitter::Node) -> bool,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for i in 0..root.child_count() as u32 {
        if let Some(child) = root.child(i) {
            if is_comment(&child) || skip(&child) {
                continue;
            }
            hash_walk(child, src, &mut hasher, None);
        }
    }
    *hasher.finalize().as_bytes()
}

fn is_comment(node: &Node) -> bool {
    node.kind().contains("comment")
}

/// Recursively feed `hasher` with `node`'s kind (+ leaf text), skipping
/// comment nodes and the node whose id equals `skip_id` (used by
/// `split_fingerprint` to exclude the body field child).
fn hash_walk(node: Node, src: &[u8], hasher: &mut blake3::Hasher, skip_id: Option<usize>) {
    if is_comment(&node) || Some(node.id()) == skip_id {
        return;
    }

    hasher.update(node.kind().as_bytes());
    hasher.update(b"\0");

    if node.child_count() == 0 {
        if let Ok(text) = node.utf8_text(src) {
            hasher.update(text.as_bytes());
            hasher.update(b"\0");
        }
    } else {
        for i in 0..node.child_count() as u32 {
            if let Some(child) = node.child(i) {
                hash_walk(child, src, hasher, skip_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts_tree(src: &str) -> (tree_sitter::Tree, Vec<u8>) {
        let mut p = tree_sitter::Parser::new();
        p.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .unwrap();
        (p.parse(src, None).unwrap(), src.as_bytes().to_vec())
    }

    #[test]
    fn comment_and_formatting_insensitive() {
        let (a, sa) = ts_tree("export function add(a: number, b: number) { return a + b; }");
        let (b, sb) = ts_tree(
            "// docs\nexport function add(a: number,\n    b: number) {\n  /* inner */ return a + b;\n}",
        );
        assert_eq!(
            fingerprint(a.root_node(), &sa),
            fingerprint(b.root_node(), &sb)
        );
    }

    #[test]
    fn token_change_changes_fingerprint() {
        let (a, sa) = ts_tree("function f() { return 1; }");
        let (b, sb) = ts_tree("function f() { return 2; }");
        assert_ne!(
            fingerprint(a.root_node(), &sa),
            fingerprint(b.root_node(), &sb)
        );
    }

    #[test]
    fn split_separates_signature_from_body() {
        let (a, sa) = ts_tree("function f(x: number) { return 1; }");
        let (b, sb) = ts_tree("function f(x: number) { return 2; }");
        let fa = a.root_node().named_child(0).unwrap();
        let fb = b.root_node().named_child(0).unwrap();
        let (sig_a, body_a) = split_fingerprint(fa, &sa);
        let (sig_b, body_b) = split_fingerprint(fb, &sb);
        assert_eq!(sig_a, sig_b);
        assert_ne!(body_a, body_b);
        assert!(body_a.is_some());
    }

    #[test]
    fn module_init_fingerprint_skips_def_bodies_but_not_other_statements() {
        let skip = |n: &tree_sitter::Node| n.kind() == "function_declaration";

        let (a, sa) = ts_tree("let x = 1;\nfunction f() { return 1; }");
        let (b, sb) = ts_tree("let x = 1;\nfunction f() { return 2; }");
        assert_eq!(
            module_init_fingerprint(a.root_node(), &sa, &skip),
            module_init_fingerprint(b.root_node(), &sb, &skip)
        );

        let (c, sc) = ts_tree("let x = 2;\nfunction f() { return 1; }");
        assert_ne!(
            module_init_fingerprint(a.root_node(), &sa, &skip),
            module_init_fingerprint(c.root_node(), &sc, &skip)
        );
    }
}
