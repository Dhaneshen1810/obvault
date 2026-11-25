use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};
fn git(p: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(p)
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap().trim().into()
}
fn cli(config: &Path, args: &[&str], input: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_obvault"))
        .env("OBVAULT_CONFIG", config)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}
#[test]
fn setup_and_restore_snapshot_preserve_history() {
    let root = std::env::temp_dir().join(format!(
        "obvault-cli-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let vault = root.join("Vault with spaces");
    fs::create_dir_all(&vault).unwrap();
    git(&vault, &["init", "-b", "main"]);
    git(&vault, &["config", "user.name", "Test"]);
    git(&vault, &["config", "user.email", "test@example.invalid"]);
    git(
        &vault,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/test/vault.git",
        ],
    );
    fs::write(vault.join("note.md"), "old").unwrap();
    git(&vault, &["add", "."]);
    git(&vault, &["commit", "-m", "first"]);
    let first = git(&vault, &["rev-parse", "HEAD"]);
    fs::write(vault.join("note.md"), "new").unwrap();
    fs::write(vault.join("added.md"), "later").unwrap();
    git(&vault, &["add", "."]);
    git(&vault, &["commit", "-m", "second"]);
    let config = root.join("config.json");
    let setup = cli(
        &config,
        &["setup"],
        &format!("{}\n\n\n3\n", vault.display()),
    );
    assert!(
        setup.status.success(),
        "{}",
        String::from_utf8_lossy(&setup.stderr)
    );
    let no = cli(&config, &["revert", &first], "n\n");
    assert!(!no.status.success());
    assert_eq!(fs::read_to_string(vault.join("note.md")).unwrap(), "new");
    let yes = cli(&config, &["revert", &first], "y\n");
    assert!(
        yes.status.success(),
        "{}",
        String::from_utf8_lossy(&yes.stderr)
    );
    assert_eq!(fs::read_to_string(vault.join("note.md")).unwrap(), "old");
    assert!(!vault.join("added.md").exists());
    assert_eq!(git(&vault, &["rev-list", "--count", "HEAD"]), "3");
    let escape = cli(&config, &["revert", &first, "--file", "../escape"], "y\n");
    assert!(!escape.status.success());
    let diff = cli(&config, &["diff"], "");
    assert!(diff.status.success());
    fs::remove_dir_all(root).unwrap();
}
