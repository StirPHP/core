use std::fs;

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::TempDir;

fn sphp() -> Command {
    Command::cargo_bin("sphp").unwrap()
}

#[test]
fn init_yes_creates_project() {
    let tmp = TempDir::new().unwrap();
    sphp()
        .current_dir(tmp.path())
        .args(["init", "--yes", "--name", "demo"])
        .assert()
        .success()
        .stdout(contains("created  sphp.toml"))
        .stdout(contains("src/main.sphp"));

    let config = fs::read_to_string(tmp.path().join("sphp.toml")).unwrap();
    assert!(config.contains("name = \"demo\""), "{config}");
    assert!(config.contains("srcDir = \"src\""), "{config}");
    assert!(tmp.path().join("src/main.sphp").is_file());
    assert_eq!(
        fs::read_to_string(tmp.path().join(".gitignore")).unwrap(),
        "/build\n"
    );

    // A second run refuses to overwrite.
    sphp()
        .current_dir(tmp.path())
        .args(["init", "--yes"])
        .assert()
        .failure()
        .stderr(contains("already exists"));

    // --force overwrites, with custom dirs.
    sphp()
        .current_dir(tmp.path())
        .args([
            "init",
            "--yes",
            "--force",
            "--src-dir",
            "app",
            "--out-dir",
            "dist",
        ])
        .assert()
        .success();
    assert!(tmp.path().join("app/main.sphp").is_file());
    assert_eq!(
        fs::read_to_string(tmp.path().join(".gitignore")).unwrap(),
        "/build\n/dist\n"
    );
}
