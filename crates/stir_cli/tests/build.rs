use std::fs;
use std::io::Read as _;
use std::path::Path;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;
use tempfile::TempDir;

fn sphp() -> Command {
    let mut cmd = Command::cargo_bin("sphp").unwrap();
    cmd.env("NO_COLOR", "1");
    cmd
}

fn project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    sphp()
        .current_dir(tmp.path())
        .args(["init", "--yes", "--name", "demo"])
        .assert()
        .success();
    tmp
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn build(dir: &Path, args: &[&str]) -> assert_cmd::assert::Assert {
    sphp().current_dir(dir).arg("build").args(args).assert()
}

fn manifest(dir: &Path) -> Vec<String> {
    let text = fs::read_to_string(dir.join(".sphp-build.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    json["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn fresh_project_builds_with_source_map() {
    let tmp = project();
    let root = tmp.path();
    build(root, &[])
        .success()
        .stdout(contains("Built 1 file (0 copied) into build/ in "));

    assert!(root.join("build/main.php").is_file());
    let files = manifest(&root.join("build"));
    assert!(files.contains(&"main.php".to_owned()));
    assert!(files.contains(&"main.php.map".to_owned()));

    let map: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("build/main.php.map")).unwrap()).unwrap();
    assert_eq!(map["version"], 3);
    assert_eq!(map["file"], "main.php");
    assert_eq!(map["sources"][0], "../src/main.sphp");
    let source = fs::read_to_string(root.join("src/main.sphp")).unwrap();
    assert_eq!(map["sourcesContent"][0], source.as_str());
    assert!(map["mappings"].is_string());
}

#[test]
fn no_source_maps() {
    let tmp = project();
    let root = tmp.path();
    build(root, &["--no-source-maps"]).success();
    assert!(root.join("build/main.php").is_file());
    assert!(!root.join("build/main.php.map").exists());

    // Turning maps off later removes the old ones (they are in the manifest).
    build(root, &[]).success();
    assert!(root.join("build/main.php.map").is_file());
    build(root, &["--no-source-maps"]).success();
    assert!(!root.join("build/main.php.map").exists());
}

#[test]
fn mirrors_src_dir_and_copies_php() {
    let tmp = project();
    let root = tmp.path();
    write(
        &root.join("src/App/Models/User.sphp"),
        "<?php\n\nnamespace App\\Models;\n\nclass User {}\n",
    );
    let legacy = "<?php\n\nnamespace App;\n\nfunction legacy() { return 1; }\n";
    write(&root.join("src/App/Legacy.php"), legacy);

    build(root, &[])
        .success()
        .stdout(contains("Built 3 files (1 copied) into build/"));

    assert!(root.join("build/App/Models/User.php").is_file());
    assert_eq!(
        fs::read_to_string(root.join("build/App/Legacy.php")).unwrap(),
        legacy
    );
    // Copied PHP gets no source map.
    assert!(!root.join("build/App/Legacy.php.map").exists());

    let map: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("build/App/Models/User.php.map")).unwrap())
            .unwrap();
    assert_eq!(map["sources"][0], "../../../src/App/Models/User.sphp");
}

#[test]
fn stale_outputs_removed_but_user_files_kept() {
    let tmp = project();
    let root = tmp.path();
    write(&root.join("src/Old.sphp"), "<?php\n\necho 1\n");
    write(&root.join("src/Sub/Gone.sphp"), "<?php\n\necho 2\n");
    build(root, &[]).success();
    assert!(root.join("build/Old.php").is_file());
    assert!(root.join("build/Sub/Gone.php").is_file());

    // Files the build did not create.
    write(&root.join("build/notes.txt"), "mine");
    write(&root.join("build/Extra.php"), "<?php\n");

    fs::remove_file(root.join("src/Old.sphp")).unwrap();
    fs::remove_dir_all(root.join("src/Sub")).unwrap();
    build(root, &[]).success();

    assert!(!root.join("build/Old.php").exists());
    assert!(!root.join("build/Old.php.map").exists());
    assert!(!root.join("build/Sub").exists(), "empty directory pruned");
    assert!(root.join("build/main.php").is_file());
    assert_eq!(
        fs::read_to_string(root.join("build/notes.txt")).unwrap(),
        "mine"
    );
    assert!(root.join("build/Extra.php").is_file());
    let files = manifest(&root.join("build"));
    assert!(
        !files
            .iter()
            .any(|f| f.starts_with("Old") || f.starts_with("Sub"))
    );
}

#[test]
fn errors_block_the_build_unless_forced() {
    let tmp = project();
    let root = tmp.path();
    write(&root.join("src/bad.sphp"), "val = \n");

    build(root, &[])
        .code(1)
        .stdout(contains("error[SPHP0001]"))
        .stdout(contains("Build failed"));
    assert!(!root.join("build").exists());

    build(root, &["--force"])
        .code(1)
        .stdout(contains("error[SPHP0001]"))
        .stdout(contains("Built 1 file"));
    assert!(root.join("build/main.php").is_file());
    assert!(!root.join("build/bad.php").is_file());
}

#[test]
fn out_dir_flag() {
    let tmp = project();
    let root = tmp.path();
    build(root, &["--out-dir", "dist"])
        .success()
        .stdout(contains("into dist/"));
    assert!(root.join("dist/main.php").is_file());
    assert!(root.join("dist/.sphp-build.json").is_file());
    assert!(!root.join("build").exists());

    // Relative to the current directory, not the project root.
    let sub = root.join("src");
    sphp()
        .current_dir(&sub)
        .args(["build", "--out-dir", "../out2"])
        .assert()
        .success();
    assert!(root.join("out2/main.php").is_file());
}

#[test]
fn unchanged_files_keep_mtime() {
    let tmp = project();
    let root = tmp.path();
    write(&root.join("src/Other.sphp"), "<?php\n\necho 1\n");
    build(root, &[]).success();
    let mtime = |p: &str| fs::metadata(root.join(p)).unwrap().modified().unwrap();
    let main = mtime("build/main.php");
    let map = mtime("build/main.php.map");
    let other = mtime("build/Other.php");
    let manifest = mtime("build/.sphp-build.json");

    std::thread::sleep(Duration::from_millis(50));
    write(&root.join("src/Other.sphp"), "<?php\n\necho 2\n");
    build(root, &[]).success();

    assert_eq!(mtime("build/main.php"), main);
    assert_eq!(mtime("build/main.php.map"), map);
    assert_eq!(mtime("build/.sphp-build.json"), manifest);
    assert_ne!(mtime("build/Other.php"), other);
}

/// Kills the child on drop so a failing assertion does not leak the watcher.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn watch_rebuilds_on_change() {
    let tmp = project();
    let root = tmp.path().to_path_buf();
    let child = std::process::Command::new(assert_cmd::cargo::cargo_bin("sphp"))
        .current_dir(&root)
        .env("NO_COLOR", "1")
        .args(["build", "--watch"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut child = KillOnDrop(child);
    // Collect stdout on a thread so the pipe never fills up.
    let output = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let mut pipe = child.0.stdout.take().unwrap();
    let sink = output.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = pipe.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    });
    let stdout = || output.lock().unwrap().clone();
    let timeout = Duration::from_secs(60);
    let read = |p: &str| fs::read_to_string(root.join(p)).unwrap_or_default();

    wait_for("initial build", timeout, || {
        root.join("build/.sphp-build.json").is_file()
    });
    // Give the watcher a moment to be fully registered.
    std::thread::sleep(Duration::from_millis(500));

    // Content change: updated in place.
    let mut main = read("src/main.sphp");
    main.push_str("echo \"watch-marker-1\"\n");
    write(&root.join("src/main.sphp"), &main);
    wait_for("rebuilt main.php", timeout, || {
        read("build/main.php").contains("watch-marker-1")
    });

    // New file: full reload.
    write(
        &root.join("src/Added.sphp"),
        "<?php\n\necho \"watch-marker-2\"\n",
    );
    wait_for("new output", timeout, || {
        read("build/Added.php").contains("watch-marker-2")
    });

    // Errors do not stop the watcher.
    write(&root.join("src/Added.sphp"), "val = \n");
    wait_for("error report", timeout, || {
        stdout().contains("error[SPHP0001]")
    });
    write(
        &root.join("src/Added.sphp"),
        "<?php\n\necho \"watch-marker-3\"\n",
    );
    wait_for("recovered build", timeout, || {
        read("build/Added.php").contains("watch-marker-3")
    });

    assert!(
        child.0.try_wait().unwrap().is_none(),
        "watcher still running"
    );
    assert!(stdout().contains("Watching"), "stdout: {}", stdout());
}

#[test]
fn strict_types_can_be_turned_off() {
    let tmp = project();
    let root = tmp.path();
    write(&root.join("src/a.sphp"), "echo \"a\"\n");
    build(root, &[]).success();
    assert!(
        fs::read_to_string(root.join("build/a.php"))
            .unwrap()
            .contains("strict_types")
    );
    let toml = fs::read_to_string(root.join("sphp.toml")).unwrap();
    write(
        &root.join("sphp.toml"),
        &format!("strictTypes = false\n{toml}"),
    );
    build(root, &[]).success();
    assert!(
        !fs::read_to_string(root.join("build/a.php"))
            .unwrap()
            .contains("strict_types")
    );
}

#[test]
fn copy_assets_makes_a_complete_tree() {
    let tmp = project();
    let root = tmp.path();
    write(&root.join("src/a.sphp"), "echo \"a\"\n");
    write(&root.join("src/css/site.css"), "body {}\n");
    build(root, &[]).success();
    assert!(!root.join("build/css/site.css").exists());
    let toml = fs::read_to_string(root.join("sphp.toml")).unwrap();
    write(
        &root.join("sphp.toml"),
        &format!("copyAssets = true\n{toml}"),
    );
    build(root, &[]).success();
    assert_eq!(
        fs::read_to_string(root.join("build/css/site.css")).unwrap(),
        "body {}\n"
    );
    assert!(root.join("build/a.php").exists());
}

#[test]
fn an_ignored_port_gets_a_warning() {
    let tmp = project();
    let root = tmp.path();
    write(&root.join(".gitignore"), "/src/assets/*\n");
    write(&root.join("src/assets/hidden.sphp"), "echo 1\n");
    sphp()
        .current_dir(root)
        .arg("check")
        .assert()
        .stderr(contains("hidden.sphp").and(contains("is ignored")));
}

#[test]
fn force_doesnt_write_a_file_that_doesnt_parse() {
    let tmp = project();
    let root = tmp.path();
    write(&root.join("src/good.sphp"), "echo 1\n");
    write(&root.join("src/broken.sphp"), "function f(: {\n");
    build(root, &["--force"]);
    assert!(root.join("build/good.php").exists());
    assert!(!root.join("build/broken.php").exists());
}
