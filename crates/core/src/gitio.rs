//! Git integration: shells out to the `git` binary via `std::process::Command`
//! rather than linking libgit2 (dependency-weight decision: see Plan 3 Task 3
//! brief). Two operations are exposed: listing changed files between two
//! revisions (or a revision and the worktree), and reading a file's content at
//! a given revision.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{bail, Context, Result};

/// How a file changed between `from` and `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed { old: PathBuf },
}

/// A single file's change between `from` and `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub status: FileStatus,
}

fn run_git(repo: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("failed to run `git` in {} (args: {args:?})", repo.display()))
}

/// Files changed between `from` and `to`. When `to` is `None`, compares
/// `from` against the worktree (both staged and unstaged changes), and also
/// reports untracked files (`git ls-files --others --exclude-standard`) as
/// `Added`, deduplicated against the diff results.
pub fn changed_files(repo: &Path, from: &str, to: Option<&str>) -> Result<Vec<ChangedFile>> {
    let mut args: Vec<&str> = vec!["diff", "--name-status", "-M", "-z", from];
    if let Some(to) = to {
        args.push(to);
    }
    let output = run_git(repo, &args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git diff {from} {to:?} failed: {}", stderr.trim());
    }
    let stdout = String::from_utf8(output.stdout)
        .context("git diff --name-status -z output is not valid UTF-8")?;
    let mut files = parse_name_status(&stdout)?;

    // Worktree mode: also surface untracked files as Added.
    if to.is_none() {
        let untracked_output =
            run_git(repo, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        if !untracked_output.status.success() {
            let stderr = String::from_utf8_lossy(&untracked_output.stderr);
            bail!("git ls-files failed: {}", stderr.trim());
        }
        let untracked = String::from_utf8(untracked_output.stdout)
            .context("git ls-files -z output is not valid UTF-8")?;
        let seen: HashSet<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
        for raw in untracked.split('\0') {
            if raw.is_empty() {
                continue;
            }
            let path = PathBuf::from(raw);
            if !seen.contains(&path) {
                files.push(ChangedFile {
                    path,
                    status: FileStatus::Added,
                });
            }
        }
    }

    Ok(files)
}

/// Parses `git diff --name-status -z` output: NUL-separated tokens, each
/// record being a status code (`M`/`A`/`D`/`R<score>`/`T`/`C<score>`)
/// followed by one path (two for `R`/`C`: old path, then new path).
///
/// Uncommon statuses degrade rather than error: `T` (type change, e.g. file
/// <-> symlink) maps to `Modified`, and `C<score>` (copy) maps to `Added` for
/// the new path; both are sound over-approximations (Item 2). `U`
/// (unmerged) and anything else unrecognized still fall through to the error
/// arm: those genuinely can't be soundly classified here.
fn parse_name_status(raw: &str) -> Result<Vec<ChangedFile>> {
    let mut tokens = raw.split('\0').filter(|s| !s.is_empty());
    let mut files = Vec::new();
    while let Some(status) = tokens.next() {
        let code = status.chars().next().unwrap_or('\0');
        match code {
            'M' => files.push(ChangedFile {
                path: PathBuf::from(next_token(&mut tokens, "M")?),
                status: FileStatus::Modified,
            }),
            'A' => files.push(ChangedFile {
                path: PathBuf::from(next_token(&mut tokens, "A")?),
                status: FileStatus::Added,
            }),
            'D' => files.push(ChangedFile {
                path: PathBuf::from(next_token(&mut tokens, "D")?),
                status: FileStatus::Deleted,
            }),
            'R' => {
                let old = next_token(&mut tokens, "R")?;
                let new = next_token(&mut tokens, "R")?;
                files.push(ChangedFile {
                    path: PathBuf::from(new),
                    status: FileStatus::Renamed {
                        old: PathBuf::from(old),
                    },
                });
            }
            'T' => files.push(ChangedFile {
                path: PathBuf::from(next_token(&mut tokens, "T")?),
                status: FileStatus::Modified,
            }),
            'C' => {
                let _old = next_token(&mut tokens, "C")?;
                let new = next_token(&mut tokens, "C")?;
                files.push(ChangedFile {
                    path: PathBuf::from(new),
                    status: FileStatus::Added,
                });
            }
            _ => bail!("git diff --name-status: unrecognized status token {status:?}"),
        }
    }
    Ok(files)
}

fn next_token<'a>(tokens: &mut impl Iterator<Item = &'a str>, code: &str) -> Result<&'a str> {
    tokens
        .next()
        .with_context(|| format!("git diff --name-status: missing path after {code} status"))
}

/// Content of `path` at `rev` (`git show rev:path`). `Ok(None)` if the path
/// does not exist at that revision; `Err` for any other failure (bad rev,
/// `git` missing, non-UTF-8 content, etc).
pub fn show_file(repo: &Path, rev: &str, path: &Path) -> Result<Option<String>> {
    let spec = format!("{rev}:{}", path.display());
    let output = run_git(repo, &["show", spec.as_str()])?;
    if output.status.success() {
        let content = String::from_utf8(output.stdout)
            .with_context(|| format!("git show {spec}: output is not valid UTF-8"))?;
        return Ok(Some(content));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("does not exist")
        || stderr.contains("exists on disk")
        || stderr.contains("fatal: path")
    {
        return Ok(None);
    }
    bail!("git show {spec} failed: {}", stderr.trim());
}

/// Resolves `rev` to its full object id (`git rev-parse --verify <rev>`),
/// used to build a collision-safe [`TempWorktree`] directory name. `Err` for
/// a rev that doesn't resolve locally (bad rev, or one that exists remotely
/// but hasn't been fetched) — the same failure shape `changed_files` already
/// has for a bad `--from`.
fn resolve_rev(repo: &Path, rev: &str) -> Result<String> {
    let output = run_git(repo, &["rev-parse", "--verify", rev])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git rev-parse --verify {rev} failed: {}", stderr.trim());
    }
    let sha =
        String::from_utf8(output.stdout).context("git rev-parse output is not valid UTF-8")?;
    Ok(sha.trim().to_string())
}

/// A temporary, detached-HEAD git worktree checked out at an arbitrary
/// revision (`git worktree add --detach <dir> <rev>`), backing `--to <rev>`
/// support: the analysis pipeline runs against this checkout as its repo
/// root instead of the caller's actual worktree, so `--from`/`--to` can name
/// any two revisions rather than only `--from` vs. the live worktree.
///
/// Lives at `$TMPDIR/testless-to-<full-sha-of-rev>-<pid>`: the sha suffix
/// (not the raw `rev` string, which might not be filesystem-safe, e.g.
/// `origin/main`) makes the location deterministic per revision, and the
/// trailing pid suffix keeps two concurrent `testless` processes analyzing
/// the *same* rev (e.g. two CI jobs racing the same PR) from colliding on
/// one worktree — each process gets its own, since `git worktree add`
/// refuses to reuse a path/registration another checkout is still using.
///
/// Removed on drop (`git worktree remove --force`, then `git worktree
/// prune`) — including on an early return via `?` anywhere between creation
/// and drop — via a hand-rolled `Drop` guard rather than an explicit cleanup
/// call at every exit path (no `scopeguard` dependency in this crate).
#[derive(Debug)]
pub struct TempWorktree {
    path: PathBuf,
    repo: PathBuf,
}

impl TempWorktree {
    /// Requires `rev` to already exist in `repo`'s local object store: a
    /// rev that hasn't been fetched surfaces as an `Err` here (see
    /// `resolve_rev`), not a silent no-op.
    pub fn create(repo: &Path, rev: &str) -> Result<Self> {
        let sha = resolve_rev(repo, rev).with_context(|| format!("resolving --to rev {rev:?}"))?;
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("testless-to-{sha}-{pid}"));

        // A leftover directory/registration from a previous crashed or
        // force-killed run would otherwise make `git worktree add` fail
        // outright and permanently wedge every future `--to <same rev>`
        // run; best-effort clear both before adding. Failures here are
        // expected (nothing to clear) and intentionally ignored.
        let _ = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "remove", "--force"])
            .arg(&path)
            .output();
        let _ = std::fs::remove_dir_all(&path);

        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "add", "--detach"])
            .arg(&path)
            .arg(rev)
            .output()
            .with_context(|| format!("failed to run `git worktree add` for rev {rev:?}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "git worktree add --detach {} {rev} failed: {}",
                path.display(),
                stderr.trim()
            );
        }

        Ok(Self {
            path,
            repo: repo.to_path_buf(),
        })
    }

    /// The checkout directory: the analysis pipeline's repo root for the
    /// `--to` rev, valid for exactly as long as `self` is alive.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempWorktree {
    fn drop(&mut self) {
        // Best-effort: a `Drop` impl can't propagate an error, and a
        // cleanup failure (e.g. the directory already gone) shouldn't panic
        // mid-unwind. `worktree remove --force` deletes the checkout
        // directory itself; `worktree prune` clears the (now-dangling)
        // registration in `repo`'s `.git/worktrees/` in the rare case
        // `remove` didn't run (e.g. the directory was already gone).
        let _ = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .output();
        let _ = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["worktree", "prune"])
            .output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::{tempdir, TempDir};

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .status()
            .expect("failed to spawn git");
        assert!(status.success(), "git {args:?} failed in {}", dir.display());
    }

    fn init_repo() -> TempDir {
        let dir = tempdir().expect("tempdir");
        git(dir.path(), &["init", "-b", "main"]);
        fs::write(dir.path().join("a.txt"), "original\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "initial"]);
        dir
    }

    #[test]
    fn changed_files_worktree_reports_modified_and_added() {
        let dir = init_repo();
        fs::write(dir.path().join("a.txt"), "changed\n").unwrap();
        fs::write(dir.path().join("b.txt"), "new\n").unwrap();

        let files = changed_files(dir.path(), "HEAD", None).expect("changed_files");

        assert_eq!(files.len(), 2, "expected a.txt + b.txt, got {files:?}");
        let a = files
            .iter()
            .find(|f| f.path == Path::new("a.txt"))
            .expect("a.txt present");
        assert_eq!(a.status, FileStatus::Modified);
        let b = files
            .iter()
            .find(|f| f.path == Path::new("b.txt"))
            .expect("b.txt present");
        assert_eq!(b.status, FileStatus::Added);
    }

    #[test]
    fn show_file_returns_content_at_rev() {
        let dir = init_repo();
        let content = show_file(dir.path(), "HEAD", Path::new("a.txt")).expect("show_file");
        assert_eq!(content, Some("original\n".to_string()));
    }

    #[test]
    fn show_file_absent_at_rev_returns_none() {
        let dir = init_repo();
        // b.txt exists on disk but was never committed, so it's absent at HEAD.
        fs::write(dir.path().join("b.txt"), "new\n").unwrap();
        let content = show_file(dir.path(), "HEAD", Path::new("b.txt")).expect("show_file");
        assert_eq!(content, None);
    }

    #[test]
    fn changed_files_bad_rev_is_err() {
        let dir = init_repo();
        let result = changed_files(dir.path(), "not-a-real-rev", None);
        assert!(result.is_err(), "expected Err for bad rev, got {result:?}");
    }

    #[test]
    fn parse_name_status_type_change_maps_to_modified() {
        let files = parse_name_status("T\0a.txt\0").expect("parse");
        assert_eq!(
            files,
            vec![ChangedFile {
                path: PathBuf::from("a.txt"),
                status: FileStatus::Modified,
            }]
        );
    }

    #[test]
    fn parse_name_status_copy_maps_to_added() {
        let files = parse_name_status("C100\0orig.txt\0copy.txt\0").expect("parse");
        assert_eq!(
            files,
            vec![ChangedFile {
                path: PathBuf::from("copy.txt"),
                status: FileStatus::Added,
            }]
        );
    }

    #[test]
    fn parse_name_status_unmerged_is_still_an_error() {
        let result = parse_name_status("U\0conflict.txt\0");
        assert!(result.is_err(), "expected Err for U status, got {result:?}");
    }

    #[test]
    fn changed_files_rev_range_detects_rename() {
        let dir = init_repo();
        git(dir.path(), &["mv", "a.txt", "renamed.txt"]);
        git(dir.path(), &["commit", "-m", "rename a.txt"]);

        let files =
            changed_files(dir.path(), "HEAD~1", Some("HEAD")).expect("changed_files rev-range");

        assert_eq!(files.len(), 1, "expected exactly one change, got {files:?}");
        assert_eq!(files[0].path, PathBuf::from("renamed.txt"));
        match &files[0].status {
            FileStatus::Renamed { old } => assert_eq!(old, &PathBuf::from("a.txt")),
            other => panic!("expected Renamed, got {other:?}"),
        }
    }

    #[test]
    fn temp_worktree_checks_out_rev_content_and_cleans_up_on_drop() {
        let dir = init_repo();
        fs::write(dir.path().join("a.txt"), "second\n").unwrap();
        git(dir.path(), &["commit", "-am", "second commit"]);

        let path = {
            let wt = TempWorktree::create(dir.path(), "HEAD~1").expect("create worktree");
            let checked_out = fs::read_to_string(wt.path().join("a.txt")).expect("read a.txt");
            assert_eq!(
                checked_out, "original\n",
                "worktree should have HEAD~1's content, not HEAD's"
            );
            assert!(wt.path().exists());
            wt.path().to_path_buf()
        };
        // Dropped at end of the block above; the checkout directory must be
        // gone, and the registration pruned (a second `create` against the
        // same rev must succeed cleanly, not collide with a stale entry).
        assert!(
            !path.exists(),
            "worktree directory should be removed after drop, still at {}",
            path.display()
        );

        let wt2 = TempWorktree::create(dir.path(), "HEAD~1").expect("recreate worktree");
        assert_eq!(wt2.path(), path, "same rev resolves to the same temp path");
    }

    #[test]
    fn temp_worktree_bad_rev_is_err() {
        let dir = init_repo();
        let result = TempWorktree::create(dir.path(), "not-a-real-rev");
        assert!(result.is_err(), "expected Err for bad rev, got {result:?}");
    }
}
