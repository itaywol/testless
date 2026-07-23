//! Def-level diff invariance tests (Plan 3, Task 2). Grows in Task 5 with
//! richer classification coverage; for now this pins down the foundational
//! guarantee `diff_defs` must uphold: comments/formatting alone must never
//! register as a change.

use testless_core::{diff_defs, Extraction, Language};
use testless_lang_ts::TsLanguage;

fn extract_ts(src: &str) -> Extraction {
    let lang = TsLanguage;
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&lang.grammar(std::path::Path::new("math.ts")))
        .unwrap();
    let tree = parser.parse(src, None).unwrap();
    lang.extract(src, &tree)
}

/// `math.ts` reformatted and commented throughout (line breaks moved,
/// indentation changed, a comment added before/inside/after every def and at
/// the top level) but with exactly the same tokens as the original fixture
/// file — no identifier, literal, or keyword changed. `diff_defs` on the two
/// extractions must report zero changes: this is the whole point of
/// structural (comment/formatting-insensitive) fingerprinting from Task 1.
const MATH_TS_REFORMATTED: &str = r#"
// header comment
export function add(
  a: number,
  b: number
): number {
  // adds two numbers
  return a + b;
}

export const mul = (a: number, b: number): number =>
  a * b; // multiply

export class Calc {
  total = 0;

  push(n: number) {
    /* accumulate */
    this.total = add(this.total, n);
  }
}

console.log("side effect at import"); // side effect
"#;

#[test]
fn comment_and_formatting_changes_yield_no_def_changes() {
    let original =
        std::fs::read_to_string("../../fixtures/ts-app/src/math.ts").expect("read fixture");

    let old = extract_ts(&original);
    let new = extract_ts(MATH_TS_REFORMATTED);

    let changes = diff_defs(&old, &new);
    assert_eq!(
        changes,
        vec![],
        "comment/formatting-only diff must be empty"
    );
}
