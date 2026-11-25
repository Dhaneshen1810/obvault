use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

/// A temporary workspace with its own Git configuration, so tests never read the
/// developer's global or system Git settings (signing, hooks, URL rewrites).
/// `https://github.com/test/NAME` is redirected to a bare repository `NAME` inside it.
struct Sandbox {
    root: PathBuf,
}
impl Sandbox {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("obvault-cli-{label}-{stamp}"));
        fs::create_dir_all(&root).unwrap();
        let s = Self { root };
        let redirect = format!("{}/", s.root.to_string_lossy().replace('\\', "/"));
        s.git(
            &s.root,
            &[
                "config",
                "--global",
                &format!("url.{redirect}.insteadOf"),
                "https://github.com/test/",
            ],
        );
        s.git(&s.root, &["config", "--global", "user.name", "Test"]);
        s.git(
            &s.root,
            &["config", "--global", "user.email", "test@example.invalid"],
        );
        s
    }
    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
    fn config(&self) -> PathBuf {
        self.path("config.json")
    }
    fn isolate(&self, cmd: &mut Command) {
        cmd.env("GIT_CONFIG_GLOBAL", self.path("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1");
    }
    fn git(&self, p: &Path, args: &[&str]) -> String {
        let mut cmd = Command::new("git");
        self.isolate(&mut cmd);
        let o = cmd.arg("-C").arg(p).args(args).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap().trim().into()
    }
    fn cli(&self, args: &[&str], input: &str) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_obvault"));
        self.isolate(&mut cmd);
        let mut child = cmd
            .env("OBVAULT_CONFIG", self.config())
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
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn text(o: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn setup_and_restore_snapshot_preserve_history() {
    let s = Sandbox::new("restore");
    let vault = s.path("Vault with spaces");
    fs::create_dir_all(&vault).unwrap();
    s.git(&vault, &["init", "-b", "main"]);
    s.git(
        &vault,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/test/vault.git",
        ],
    );
    fs::write(vault.join("note.md"), "old").unwrap();
    s.git(&vault, &["add", "."]);
    s.git(&vault, &["commit", "-m", "first"]);
    let first = s.git(&vault, &["rev-parse", "HEAD"]);
    fs::write(vault.join("note.md"), "new").unwrap();
    fs::write(vault.join("added.md"), "later").unwrap();
    s.git(&vault, &["add", "."]);
    s.git(&vault, &["commit", "-m", "second"]);
    s.git(&s.root, &["init", "--bare", "vault.git"]);
    let setup = s.cli(
        &["setup"],
        &format!(
            "invalid\n3\n{}\n{}\nbad-url\nhttps://github.com/test/missing.git\n\nbad branch\nwrong-branch\nmain\n",
            s.path("missing").display(),
            vault.display()
        ),
    );
    assert!(setup.status.success(), "{}", text(&setup));
    let output = String::from_utf8_lossy(&setup.stdout);
    for expected in [
        "Choose 1, 2, or 3",
        "Local vault folder does not exist",
        "Use https://github.com/OWNER/REPO",
        "Cannot access repository",
        "Local repository is on branch",
        "OK: Workflow validated",
        "OK: Local vault path",
        "OK: GitHub repository",
        "OK: Branch validated",
    ] {
        assert!(output.contains(expected), "Missing {expected}: {output}");
    }
    let saved = fs::read(s.config()).unwrap();
    let cancelled = s.cli(&["setup"], &format!("3\n{}\n", s.path("missing").display()));
    assert!(!cancelled.status.success());
    assert_eq!(fs::read(s.config()).unwrap(), saved);
    let no = s.cli(&["revert", &first], "n\n");
    assert!(!no.status.success());
    assert_eq!(fs::read_to_string(vault.join("note.md")).unwrap(), "new");
    let yes = s.cli(&["revert", &first], "invalid\ny\n");
    assert!(yes.status.success(), "{}", text(&yes));
    assert!(text(&yes).contains("Enter y or n."));
    assert_eq!(fs::read_to_string(vault.join("note.md")).unwrap(), "old");
    assert!(!vault.join("added.md").exists());
    assert_eq!(s.git(&vault, &["rev-list", "--count", "HEAD"]), "3");
    let escape = s.cli(&["revert", &first, "--file", "../escape"], "y\n");
    assert!(!escape.status.success());
    let diff = s.cli(&["diff"], "");
    assert!(diff.status.success());
}

#[test]
fn declining_upload_leaves_vault_untouched() {
    let s = Sandbox::new("decline");
    let vault = s.path("vault");
    fs::create_dir_all(&vault).unwrap();
    fs::write(vault.join("note.md"), "hi").unwrap();
    s.git(&s.root, &["init", "--bare", "remote.git"]);
    let out = s.cli(
        &["setup"],
        &format!(
            "1\n{}\nhttps://github.com/test/remote.git\nmain\nn\n",
            vault.display()
        ),
    );
    assert!(!out.status.success(), "{}", text(&out));
    let shown = text(&out);
    assert_eq!(
        shown.matches("Upload local vault files?").count(),
        1,
        "{shown}"
    );
    assert!(!shown.contains("OK: Upload"), "{shown}");
    assert!(
        !vault.join(".git").exists(),
        "setup left a repository behind"
    );
    assert!(!vault.join(".gitignore").exists());
    assert!(!s.config().exists());
}

#[test]
fn declining_upload_restores_existing_origin() {
    let s = Sandbox::new("origin");
    let vault = s.path("vault");
    fs::create_dir_all(&vault).unwrap();
    s.git(&vault, &["init", "-b", "main"]);
    s.git(
        &vault,
        &["remote", "add", "origin", "https://github.com/test/old.git"],
    );
    s.git(&s.root, &["init", "--bare", "remote.git"]);
    let out = s.cli(
        &["setup"],
        &format!(
            "1\n{}\nhttps://github.com/test/remote.git\nmain\ny\nn\n",
            vault.display()
        ),
    );
    assert!(!out.status.success(), "{}", text(&out));
    assert_eq!(
        s.git(&vault, &["config", "--get", "remote.origin.url"]),
        "https://github.com/test/old.git"
    );
    assert!(vault.join(".git").exists());
}

#[test]
fn upload_into_new_github_repo_keeps_starter_files() {
    let s = Sandbox::new("starter");
    let remote = s.path("remote.git");
    s.git(&s.root, &["init", "--bare", "remote.git"]);
    // A repository created on GitHub with "Add a README" and a license.
    let seed = s.path("seed");
    fs::create_dir_all(&seed).unwrap();
    s.git(&seed, &["init", "-b", "main"]);
    fs::write(seed.join("README.md"), "# remote readme\n").unwrap();
    fs::write(seed.join("LICENSE"), "license\n").unwrap();
    s.git(&seed, &["add", "."]);
    s.git(&seed, &["commit", "-m", "Initial commit"]);
    let seed_commit = s.git(&seed, &["rev-parse", "HEAD"]);
    s.git(&seed, &["push", remote.to_str().unwrap(), "main"]);

    let vault = s.path("vault");
    fs::create_dir_all(&vault).unwrap();
    fs::write(vault.join("note.md"), "hi\n").unwrap();
    fs::write(vault.join("README.md"), "# my vault\n").unwrap();
    let out = s.cli(
        &["setup"],
        &format!(
            "1\n{}\nhttps://github.com/test/remote.git\nmain\ny\n",
            vault.display()
        ),
    );
    assert!(out.status.success(), "{}", text(&out));
    let shown = text(&out);
    assert!(shown.contains("only has starter files"), "{shown}");
    assert!(!shown.contains("REPLACE"), "{shown}");

    let files = s.git(&remote, &["ls-tree", "-r", "--name-only", "main"]);
    for f in ["LICENSE", "README.md", "note.md", ".gitignore"] {
        assert!(files.lines().any(|l| l == f), "missing {f}: {files}");
    }
    assert_eq!(s.git(&remote, &["show", "main:README.md"]), "# my vault");
    // The GitHub history was extended, not replaced, and no backup branch was needed.
    s.git(
        &remote,
        &["merge-base", "--is-ancestor", &seed_commit, "main"],
    );
    assert_eq!(
        s.git(&remote, &["branch", "--list", "obvault-backup-*"]),
        ""
    );
}

#[test]
fn upload_over_unrelated_vault_requires_confirmed_replacement() {
    let s = Sandbox::new("replace");
    let remote = s.path("remote.git");
    s.git(&s.root, &["init", "--bare", "remote.git"]);
    let other = s.path("other");
    fs::create_dir_all(&other).unwrap();
    s.git(&other, &["init", "-b", "main"]);
    fs::write(other.join("someone-else.md"), "notes\n").unwrap();
    s.git(&other, &["add", "."]);
    s.git(&other, &["commit", "-m", "other vault"]);
    let old = s.git(&other, &["rev-parse", "HEAD"]);
    s.git(&other, &["push", remote.to_str().unwrap(), "main"]);

    let vault = s.path("vault");
    fs::create_dir_all(&vault).unwrap();
    fs::write(vault.join("note.md"), "hi\n").unwrap();
    let input = format!(
        "1\n{}\nhttps://github.com/test/remote.git\nmain\n",
        vault.display()
    );
    let declined = s.cli(&["setup"], &format!("{input}n\n"));
    assert!(!declined.status.success());
    assert!(text(&declined).contains("REPLACE"));
    assert!(!vault.join(".git").exists());
    assert_eq!(s.git(&remote, &["rev-parse", "main"]), old);

    let accepted = s.cli(&["setup"], &format!("{input}y\ny\n"));
    assert!(accepted.status.success(), "{}", text(&accepted));
    let backup = s.git(
        &remote,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "refs/heads/obvault-backup-*",
        ],
    );
    assert_eq!(backup, old);
    assert_eq!(s.git(&remote, &["show", "main:note.md"]), "hi");
}
