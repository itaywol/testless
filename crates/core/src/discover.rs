use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::language::{Language, Registry};

/// Walk `root`, respecting `.gitignore` and hard-skipping known noise
/// directories, returning repo-relative sorted paths matched by `registry`.
pub fn discover<'r>(root: &Path, registry: &'r Registry) -> Vec<(PathBuf, &'r dyn Language)> {
    const SKIP_DIRS: &[&str] = &["node_modules", "vendor", "target", ".pick-a-test"];

    let mut out = Vec::new();

    let walker = WalkBuilder::new(root)
        .require_git(false)
        .hidden(true)
        .filter_entry(|entry| {
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                let is_skipped = entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| SKIP_DIRS.contains(&name));
                !is_skipped
            } else {
                true
            }
        })
        .build();

    for entry in walker.filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        if let Some(lang) = registry.for_path(path) {
            let rel = path.strip_prefix(root).expect("entry under root").to_path_buf();
            out.push((rel, lang));
        }
    }

    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::tests_support::Fake;

    #[test]
    fn discovers_matching_files_respecting_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/dep")).unwrap();
        std::fs::write(root.join("src/a.fk"), "").unwrap();
        std::fs::write(root.join("src/z.fk"), "").unwrap();
        std::fs::write(root.join("lib/b.fk"), "").unwrap();
        std::fs::write(root.join("src/skip.txt"), "").unwrap();
        std::fs::write(root.join("node_modules/dep/b.fk"), "").unwrap();
        std::fs::write(root.join("ignored.fk"), "").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored.fk\n").unwrap();

        let r = Registry::new(vec![Box::new(Fake)]);
        let found: Vec<_> = discover(root, &r).into_iter().map(|(p, _)| p).collect();
        assert_eq!(
            found,
            vec![
                std::path::PathBuf::from("lib/b.fk"),
                std::path::PathBuf::from("src/a.fk"),
                std::path::PathBuf::from("src/z.fk"),
            ]
        );
    }
}
