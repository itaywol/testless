use assert_cmd::Command;
use predicates::prelude::*;

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() { copy_dir(&e.path(), &to); }
        else { std::fs::copy(e.path(), &to).unwrap(); }
    }
}

fn tmp_fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    copy_dir(std::path::Path::new("../../fixtures/ts-app"), tmp.path());
    tmp
}

#[test]
fn index_then_stats_json() {
    let tmp = tmp_fixture();
    Command::cargo_bin("testless").unwrap()
        .arg("index").current_dir(tmp.path())
        .assert().success()
        .stdout(predicate::str::contains("\"defs\""));   // piped => JSON
    Command::cargo_bin("testless").unwrap()
        .arg("stats").current_dir(tmp.path())
        .assert().success()
        .stdout(predicate::str::contains("\"tests\""));
}

#[test]
fn stats_without_index_hints() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("testless").unwrap()
        .arg("stats").current_dir(tmp.path())
        .assert().failure()
        .stderr(predicate::str::contains("testless index"));
}

#[test]
fn index_twice_reuses_cache() {
    let tmp = tmp_fixture();
    let first = Command::cargo_bin("testless").unwrap()
        .arg("index").current_dir(tmp.path())
        .assert().success();
    let first_out = String::from_utf8(first.get_output().stdout.clone()).unwrap();
    assert!(first_out.contains("\"parsed\""), "first run output: {first_out}");

    let second = Command::cargo_bin("testless").unwrap()
        .arg("index").current_dir(tmp.path())
        .assert().success();
    let second_out = String::from_utf8(second.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(second_out.trim()).unwrap();
    assert_eq!(json["parsed"], 0, "second run output: {second_out}");
}
