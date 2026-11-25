use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Clone, Serialize, Deserialize)]
struct Vault {
    path: PathBuf,
    repo: String,
    branch: String,
}
#[derive(Default, Serialize, Deserialize)]
struct Config {
    default: String,
    vaults: BTreeMap<String, Vault>,
}
fn fail<T>(s: &str) -> Result<T> {
    Err(s.into())
}
fn git(path: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()?;
    if !out.status.success() {
        return fail(&format!(
            "git {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8(out.stdout)?.trim_end().to_owned())
}
fn show(path: &Path, args: &[&str]) -> Result<()> {
    if !Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .status()?
        .success()
    {
        return fail("Git command failed.");
    }
    Ok(())
}
fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}
fn config_path() -> Result<PathBuf> {
    if let Some(p) = env::var_os("OBVAULT_CONFIG") {
        return Ok(p.into());
    }
    let base = if cfg!(windows) {
        env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        env::var_os("HOME").map(|x| PathBuf::from(x).join("Library/Application Support"))
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|x| PathBuf::from(x).join(".config")))
    };
    Ok(base
        .ok_or("Cannot find user configuration directory")?
        .join("obvault/config.json"))
}
fn read_config(p: &Path) -> Result<Config> {
    if p.exists() {
        Ok(serde_json::from_slice(&fs::read(p)?)?)
    } else {
        Ok(Config::default())
    }
}
fn save_config(p: &Path, c: &Config) -> Result<()> {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = p.with_extension(format!("{}.tmp", stamp()));
    fs::write(&tmp, serde_json::to_vec_pretty(c)?)?;
    // Windows cannot rename over an existing file. Preserve the old configuration until writing succeeds.
    if cfg!(windows) && p.exists() {
        fs::copy(p, p.with_extension("json.bak"))?;
        fs::remove_file(p)?;
    }
    fs::rename(tmp, p)?;
    Ok(())
}
fn prompt(label: &str, default: &str) -> Result<String> {
    print!(
        "{label}{}: ",
        if default.is_empty() {
            String::new()
        } else {
            format!(" [{default}]")
        }
    );
    io::stdout().flush()?;
    let mut s = String::new();
    if io::stdin().read_line(&mut s)? == 0 {
        return fail("Input ended; setup cancelled.");
    }
    Ok(if s.trim().is_empty() {
        default.to_owned()
    } else {
        s.trim().to_owned()
    })
}
fn confirm(text: &str) -> Result<()> {
    let accepted = validated(&format!("{text}\nContinue? y/N"), "N", |input| match input
        .to_ascii_lowercase()
        .as_str()
    {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => fail("Enter y or n."),
    })?;
    if accepted { Ok(()) } else { fail("Cancelled.") }
}
fn expand(s: &str) -> PathBuf {
    if s == "~" || s.starts_with("~/") {
        if let Some(h) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
            return PathBuf::from(h).join(s.strip_prefix("~/").unwrap_or(""));
        }
    }
    PathBuf::from(s)
}
fn valid_repo(s: &str) -> bool {
    let rest = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("git@github.com:"))
        .or_else(|| s.strip_prefix("ssh://git@github.com/"));
    rest.is_some_and(|r| {
        let r = r.strip_suffix(".git").unwrap_or(r);
        let parts: Vec<_> = r.split('/').collect();
        parts.len() == 2
            && parts.iter().all(|p| {
                !p.is_empty()
                    && *p != "."
                    && *p != ".."
                    && p.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            })
    })
}
fn branch(v: &Vault) -> Result<()> {
    let b = git(&v.path, &["symbolic-ref", "--short", "HEAD"])?;
    if b != v.branch {
        return fail(
            "Current branch differs from configured branch. Run setup or switch branches first.",
        );
    }
    Ok(())
}
fn ready(v: &Vault) -> Result<()> {
    if !v.path.join(".git").exists() {
        return fail("Configured vault is not a Git repository.");
    }
    for marker in [
        "MERGE_HEAD",
        "rebase-merge",
        "rebase-apply",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
    ] {
        let p = git(&v.path, &["rev-parse", "--git-path", marker])?;
        let p = PathBuf::from(p);
        if (if p.is_absolute() { p } else { v.path.join(p) }).exists() {
            return fail(
                "A Git operation is unfinished. Resolve or abort it before using obvault.",
            );
        }
    }
    if git(&v.path, &["config", "--get", "remote.origin.url"])? != v.repo {
        return fail("Origin changed. Run setup again.");
    }
    branch(v)
}
fn clean(v: &Vault) -> Result<()> {
    if !git(&v.path, &["status", "--porcelain"])?.is_empty() {
        return fail(
            "Local edits exist. Use obvault diff and obvault push before pulling or restoring.",
        );
    }
    Ok(())
}
fn fetch(v: &Vault) -> Result<()> {
    git(&v.path, &["fetch", "origin"])?;
    Ok(())
}
fn remote(v: &Vault) -> String {
    format!("refs/remotes/origin/{}", v.branch)
}
fn remote_oid(v: &Vault) -> Result<Option<String>> {
    let output = git(
        &v.path,
        &[
            "ls-remote",
            "--heads",
            "origin",
            &format!("refs/heads/{}", v.branch),
        ],
    )?;
    Ok(output.split_whitespace().next().map(str::to_owned))
}
fn ahead_behind(v: &Vault) -> Result<(usize, usize)> {
    let counts = git(
        &v.path,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("HEAD...{}", remote(v)),
        ],
    )?;
    let mut n = counts.split_whitespace();
    Ok((
        n.next().ok_or("Missing count")?.parse()?,
        n.next().ok_or("Missing count")?.parse()?,
    ))
}
fn status(v: &Vault) -> Result<()> {
    ready(v)?;
    show(&v.path, &["status", "--short"])?;
    let dirty = !git(&v.path, &["status", "--porcelain"])?.is_empty();
    if let Err(e) = fetch(v) {
        println!("Remote status unknown (offline or authentication failed): {e}");
        return Ok(());
    }
    if remote_oid(v)?.is_none() {
        println!("Remote branch does not exist yet. Run obvault push.");
        return Ok(());
    }
    let (ahead, behind) = ahead_behind(v)?;
    println!("{ahead} local commit(s) to push; {behind} remote commit(s) to pull.");
    if !dirty && ahead == 0 && behind == 0 {
        println!("In sync.");
    } else if dirty {
        println!("Uncommitted local changes exist.");
    }
    Ok(())
}
fn push(v: &Vault, message: &str) -> Result<()> {
    ready(v)?;
    fetch(v)?;
    if remote_oid(v)?.is_some()
        && !Command::new("git")
            .arg("-C")
            .arg(&v.path)
            .args(["merge-base", "--is-ancestor", &remote(v), "HEAD"])
            .status()?
            .success()
    {
        return fail(
            "GitHub has changes not present locally. Pull first; diverged histories require manual resolution. No files were changed.",
        );
    }
    if !git(&v.path, &["status", "--porcelain"])?.is_empty() {
        git(&v.path, &["add", "--all"])?;
        git(&v.path, &["commit", "-m", message])?;
    }
    show(
        &v.path,
        &[
            "push",
            "-u",
            "origin",
            &format!("HEAD:refs/heads/{}", v.branch),
        ],
    )
}
fn pull(v: &Vault) -> Result<()> {
    ready(v)?;
    clean(v)?;
    fetch(v)?;
    show(&v.path, &["merge", "--ff-only", &remote(v)])
}
fn replace_remote(v: &Vault, old: &str) -> Result<String> {
    if remote_oid(v)?.as_deref() != Some(old) {
        return fail("Remote branch changed; setup replacement stopped.");
    }
    let backup = format!("obvault-backup-{}", stamp());
    // Fetch the exact inspected commit before backing it up.
    git(&v.path, &["fetch", "origin", old])?;
    git(
        &v.path,
        &[
            "push",
            &format!("--force-with-lease=refs/heads/{backup}:"),
            "origin",
            &format!("{old}:refs/heads/{backup}"),
        ],
    )?;
    let check = git(
        &v.path,
        &[
            "ls-remote",
            "--heads",
            "origin",
            &format!("refs/heads/{backup}"),
        ],
    )?;
    if check.split_whitespace().next() != Some(old) {
        return fail("Backup verification failed. Remote main branch was not replaced.");
    }
    git(
        &v.path,
        &[
            "push",
            &format!("--force-with-lease=refs/heads/{}:{old}", v.branch),
            "-u",
            "origin",
            &format!("HEAD:refs/heads/{}", v.branch),
        ],
    )?;
    Ok(backup)
}
fn ignore(path: &Path) -> Result<()> {
    let p = path.join(".gitignore");
    let mut s = if p.exists() {
        fs::read_to_string(&p)?
    } else {
        String::new()
    };
    if !s.ends_with('\n') {
        s.push('\n');
    }
    for rule in [
        ".obsidian/",
        ".trash/",
        ".DS_Store",
        ".env",
        ".env.*",
        "*.pem",
        "*.key",
    ] {
        if !s.lines().any(|l| l == rule) {
            s.push_str(rule);
            s.push('\n');
        }
    }
    fs::write(p, s)?;
    Ok(())
}
fn discovered() -> Vec<PathBuf> {
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    let base = if cfg!(windows) {
        env::var_os("APPDATA").map(PathBuf::from).unwrap_or(home)
    } else if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or(home.join(".config"))
    };
    let value = fs::read(base.join("obsidian/obsidian.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    value
        .and_then(|v| v["vaults"].as_object().cloned())
        .map(|m| {
            m.values()
                .filter_map(|v| v["path"].as_str().map(PathBuf::from))
                .collect()
        })
        .unwrap_or_default()
}
fn validated<T>(label: &str, default: &str, mut check: impl FnMut(&str) -> Result<T>) -> Result<T> {
    loop {
        let input = prompt(label, default)?;
        match check(&input) {
            Ok(value) => {
                println!("OK: {label} validated.");
                return Ok(value);
            }
            Err(error) => println!("Invalid entry: {error}\nPlease try again (Ctrl+C to cancel)."),
        }
    }
}
fn repository_refs(path: &Path, repo: &str) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["ls-remote", "--heads", "--", repo])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never")
        .output()?;
    if !out.status.success() {
        return fail(&format!(
            "Cannot access repository. Check its URL, network and Git authentication. {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8(out.stdout)?)
}
fn setup(c: &mut Config, cp: &Path, name: &str) -> Result<()> {
    let existing = c.vaults.get(name).cloned();
    println!(
        "1. Upload local vault to GitHub\n2. Download GitHub vault into a new or empty folder\n3. Connect existing local Git repository (configuration only)"
    );
    let mode = validated(
        "Workflow",
        if existing.is_some() { "3" } else { "1" },
        |input| {
            if ["1", "2", "3"].contains(&input) {
                Ok(input.to_owned())
            } else {
                fail("Choose 1, 2, or 3.")
            }
        },
    )?;
    let found = discovered();
    for (i, p) in found.iter().enumerate() {
        println!("{}. {}", i + 1, p.display());
    }
    let default = existing
        .as_ref()
        .map(|v| v.path.display().to_string())
        .unwrap_or_default();
    let path = validated("Local vault path (or listed number)", &default, |input| {
        if input.is_empty() {
            return fail("Vault path is required.");
        }
        let path = if let Ok(number) = input.parse::<usize>() {
            number
                .checked_sub(1)
                .and_then(|i| found.get(i).cloned())
                .ok_or("Choose a listed number or enter a folder path.")?
        } else {
            expand(input)
        };
        if mode == "2" {
            if path.exists() {
                if !path.is_dir() {
                    return fail("Destination must be a folder, not a file.");
                }
                if fs::read_dir(&path)?.next().is_some() {
                    return fail("Download requires a new or empty destination folder.");
                }
            } else {
                let absolute = if path.is_absolute() {
                    path.clone()
                } else {
                    env::current_dir()?.join(&path)
                };
                let mut parent = absolute.parent().ok_or("Invalid destination path")?;
                while !parent.exists() {
                    parent = parent
                        .parent()
                        .ok_or("Destination has no existing parent folder")?;
                }
                if !parent.is_dir() {
                    return fail("Destination's parent is not a folder.");
                }
            }
        } else {
            if !path.is_dir() {
                return fail("Local vault folder does not exist or is not a folder.");
            }
            fs::read_dir(&path)?;
            if mode == "3" && !path.join(".git").exists() {
                return fail("No Git repository found at this folder. Choose the repository root.");
            }
        }
        Ok(path)
    })?;
    let detected = if path.join(".git").exists() {
        git(&path, &["config", "--get", "remote.origin.url"]).unwrap_or_default()
    } else {
        String::new()
    };
    let (repo, refs) = validated(
        "GitHub repository HTTPS or SSH URL",
        existing
            .as_ref()
            .map(|v| v.repo.as_str())
            .unwrap_or(&detected),
        |input| {
            if !valid_repo(input) {
                return fail("Use https://github.com/OWNER/REPO or git@github.com:OWNER/REPO.git.");
            }
            let cwd = env::current_dir()?;
            let refs = repository_refs(if path.is_dir() { &path } else { &cwd }, input)?;
            Ok((input.to_owned(), refs))
        },
    )?;
    let local_branch = if path.join(".git").exists() {
        Some(git(&path, &["symbolic-ref", "--short", "HEAD"])?)
    } else {
        None
    };
    let default_branch = local_branch.as_deref().unwrap_or("main");
    let b = validated(
        "Branch",
        existing
            .as_ref()
            .map(|v| v.branch.as_str())
            .unwrap_or(default_branch),
        |input| {
            git(Path::new("."), &["check-ref-format", "--branch", input])?;
            let remote_branch = format!("refs/heads/{input}");
            if mode == "2"
                && !refs
                    .lines()
                    .any(|line| line.split_whitespace().nth(1) == Some(remote_branch.as_str()))
            {
                return fail(
                    "This branch does not exist on GitHub. Enter an existing remote branch.",
                );
            }
            if mode != "2"
                && local_branch
                    .as_ref()
                    .is_some_and(|current| current != input)
            {
                return fail(&format!(
                    "Local repository is on branch '{}'. Enter that branch, or cancel and switch branches using Git first.",
                    local_branch.as_deref().unwrap()
                ));
            }
            Ok(input.to_owned())
        },
    )?;
    if mode == "2" {
        if path.exists() && fs::read_dir(&path)?.next().is_some() {
            return fail("Download requires a new or empty destination folder.");
        }
        if !Command::new("git")
            .args(["clone", "--branch", &b, "--", &repo])
            .arg(&path)
            .status()?
            .success()
        {
            return fail("Clone failed; configuration was not saved.");
        }
    } else {
        if !path.is_dir() {
            return fail("Local vault folder does not exist.");
        }
        if !path.join(".git").exists() {
            if mode == "3" {
                return fail("No Git repository found at this folder.");
            }
            git(&path, &["init", "-b", &b])?;
        }
        if !detected.is_empty() && detected != repo {
            confirm(&format!("Change origin from {detected} to {repo}?"))?;
            git(&path, &["remote", "set-url", "origin", &repo])?;
        } else if detected.is_empty() {
            git(&path, &["remote", "add", "origin", &repo])?;
        }
    }
    let v = Vault {
        path: fs::canonicalize(path)?,
        repo,
        branch: b,
    };
    ready(&v)?;
    println!("OK: Local repository connected.");
    if mode == "1" {
        fetch(&v)?;
        let old = remote_oid(&v)?;
        let replace = if let Some(ref oid) = old {
            let compatible = Command::new("git")
                .arg("-C")
                .arg(&v.path)
                .args(["merge-base", "--is-ancestor", oid, "HEAD"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?
                .success();
            if !compatible {
                confirm(&format!(
                    "Use your local vault as the starting version on GitHub?\nRepository: {}\nBranch: {}\nThis will REPLACE this branch's files and history with your local vault.\nExisting remote history will first be saved and verified on obvault-backup-<timestamp>.\nIf backup fails or the remote branch changes, replacement stops.",
                    v.repo, v.branch
                ))?;
                true
            } else {
                false
            }
        } else {
            false
        };
        confirm(
            "Upload local vault files? Local .obsidian settings, trash, and common secret files will be ignored. Already tracked files remain tracked; review them with Git first.",
        )?;
        ignore(&v.path)?;
        git(&v.path, &["add", "--all"])?;
        if !git(&v.path, &["diff", "--cached", "--name-only"])?.is_empty() {
            git(
                &v.path,
                &["commit", "-m", "Set up Obsidian vault with obvault"],
            )?;
        }
        if replace {
            println!(
                "Remote history preserved on {}",
                replace_remote(&v, old.as_deref().unwrap())?
            );
        } else {
            push(&v, "Set up vault")?;
        }
    }
    c.vaults.insert(name.into(), v);
    if c.default.is_empty() {
        c.default = name.into();
    }
    save_config(cp, c)?;
    println!(
        "Saved vault '{name}'. Run obvault status. Future configuration changes: obvault setup."
    );
    Ok(())
}
fn restore(v: &Vault, version: &str, file: Option<&str>) -> Result<()> {
    ready(v)?;
    clean(v)?;
    let oid = git(
        &v.path,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{version}^{{commit}}"),
        ],
    )?;
    if let Some(f) = file {
        let p = Path::new(f);
        if p.is_absolute()
            || p.components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            || p.components().any(|c| c.as_os_str() == ".git")
        {
            return fail("Use a vault-relative file path without '..' or '.git'.");
        }
        git(&v.path, &["cat-file", "-e", &format!("{oid}:{f}")])?;
    }
    let target = file.unwrap_or(".");
    show(
        &v.path,
        &["--literal-pathspecs", "diff", "HEAD", &oid, "--", target],
    )?;
    confirm(&format!(
        "Restore {target} from {oid}? This creates a new local commit; history is preserved."
    ))?;
    git(
        &v.path,
        &[
            "--literal-pathspecs",
            "restore",
            "--source",
            &oid,
            "--staged",
            "--worktree",
            "--",
            target,
        ],
    )?;
    if git(&v.path, &["diff", "--cached", "--name-only"])?.is_empty() {
        println!("Already matches that version.");
        return Ok(());
    }
    git(
        &v.path,
        &["commit", "-m", &format!("Restore {target} from {oid}")],
    )?;
    println!("Restored locally. Run obvault push to upload.");
    Ok(())
}
fn open(v: &Vault) -> Result<()> {
    let path = v.path.to_str().ok_or("Vault path is not UTF-8")?;
    let encoded: String = path
        .as_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(b) {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let url = format!("obsidian://open?path={encoded}");
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(&url);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("rundll32.exe");
        c.args(["url.dll,FileProtocolHandler", &url]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(&url);
        c
    };
    if !cmd.status()?.success() {
        return fail("Could not open Obsidian. Install it and register its URL handler.");
    }
    Ok(())
}
fn run() -> Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let mut name = None;
    if let Some(i) = args.iter().position(|s| s == "--vault") {
        if i + 1 >= args.len() {
            return fail("--vault needs a name");
        }
        name = Some(args.remove(i + 1));
        args.remove(i);
    }
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    if ["help", "--help", "-h"].contains(&cmd) {
        println!(
            "obvault — Git versioning for Obsidian\n\nCommands:\n  setup                       Configure or update a vault interactively\n  status                      Fetch and report local/remote state\n  diff [--remote]             Show local edits or compare files with GitHub\n  push [-m MESSAGE]           Commit local edits and upload\n  pull                        Apply remote updates (fast-forward only)\n  history                     List saved versions\n  revert VERSION [--file PATH] Restore a snapshot as a new commit\n  open                        Open the vault in Obsidian\n\nGlobal: --vault NAME (default: configured default)\nRequirements: Git, existing GitHub authentication; Obsidian for open."
        );
        return Ok(());
    }
    if cmd == "--version" {
        println!("obvault {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let cp = config_path()?;
    let mut c = read_config(&cp)?;
    let name = name.unwrap_or_else(|| {
        if c.default.is_empty() {
            "default".into()
        } else {
            c.default.clone()
        }
    });
    if cmd == "setup" {
        if args.len() != 1 {
            return fail("Usage: obvault setup [--vault NAME]");
        }
        return setup(&mut c, &cp, &name);
    }
    let v = c
        .vaults
        .get(&name)
        .ok_or("Vault not configured. Run obvault setup.")?;
    match cmd {
        "status" if args.len() == 1 => status(v),
        "diff" if args.len() == 1 => {
            ready(v)?;
            show(&v.path, &["diff", "HEAD"])?;
            if git(&v.path, &["status", "--porcelain"])?.is_empty() {
                println!("No local changes.");
            }
            let files = git(&v.path, &["ls-files", "--others", "--exclude-standard"])?;
            if !files.is_empty() {
                println!("Untracked files (not included in text diff):\n{files}");
            }
            Ok(())
        }
        "diff" if args.len() == 2 && args[1] == "--remote" => {
            ready(v)?;
            fetch(v)?;
            show(&v.path, &["diff", &remote(v)])?;
            let changed = git(&v.path, &["diff", "--name-only", &remote(v)])?;
            let untracked = git(&v.path, &["ls-files", "--others", "--exclude-standard"])?;
            if !untracked.is_empty() {
                println!("Untracked local files (not included in text diff):\n{untracked}");
            }
            if changed.is_empty() && untracked.is_empty() {
                println!("Local files match GitHub. Run status to compare commit histories.");
            }
            Ok(())
        }
        "push" if args.len() == 1 => push(v, &format!("Vault update {}", stamp())),
        "push" if args.len() == 3 && args[1] == "-m" => push(v, &args[2]),
        "pull" if args.len() == 1 => pull(v),
        "history" if args.len() == 1 => {
            ready(v)?;
            show(
                &v.path,
                &["log", "-30", "--date=local", "--format=%h %ad %s"],
            )
        }
        "revert" if args.len() == 2 => restore(v, &args[1], None),
        "revert" if args.len() == 4 && args[2] == "--file" => restore(v, &args[1], Some(&args[3])),
        "open" if args.len() == 1 => open(v),
        _ => fail("Unknown command or arguments. Run obvault --help."),
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("obvault: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Sandbox(PathBuf);
    impl Sandbox {
        fn new() -> Self {
            let p =
                env::temp_dir().join(format!("obvault-test-{}-{}", std::process::id(), stamp()));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn init(p: &Path) {
        fs::create_dir_all(p).unwrap();
        git(p, &["init", "-b", "main"]).unwrap();
        git(p, &["config", "user.name", "Test"]).unwrap();
        git(p, &["config", "user.email", "test@example.invalid"]).unwrap();
    }
    fn commit(p: &Path, text: &str) -> String {
        fs::write(p.join("note.md"), text).unwrap();
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-m", "test"]).unwrap();
        git(p, &["rev-parse", "HEAD"]).unwrap()
    }
    #[test]
    fn validates_urls() {
        assert!(valid_repo("git@github.com:owner/vault.git"));
        assert!(valid_repo("https://github.com/owner/vault"));
        for s in [
            "https://github.com.evil/x/y",
            "--upload-pack=evil",
            "https://token@github.com/x/y",
            "https://github.com/a/../b",
        ] {
            assert!(!valid_repo(s));
        }
    }
    #[test]
    fn config_round_trip() {
        let t = Sandbox::new();
        let p = t.0.join("config.json");
        let mut c = Config {
            default: "work".into(),
            ..Default::default()
        };
        c.vaults.insert(
            "work".into(),
            Vault {
                path: t.0.join("Vault with spaces"),
                repo: "https://github.com/test/vault".into(),
                branch: "main".into(),
            },
        );
        save_config(&p, &c).unwrap();
        save_config(&p, &c).unwrap();
        assert_eq!(
            read_config(&p).unwrap().vaults["work"].path,
            c.vaults["work"].path
        );
    }
    #[test]
    fn push_pull_and_backup_preserve_remote_history() {
        let t = Sandbox::new();
        let local = t.0.join("local");
        let remote_path = t.0.join("remote.git");
        let other = t.0.join("other");
        git(&t.0, &["init", "--bare", remote_path.to_str().unwrap()]).unwrap();
        init(&local);
        commit(&local, "first");
        let url = remote_path.to_str().unwrap();
        git(&local, &["remote", "add", "origin", url]).unwrap();
        let v = Vault {
            path: local.clone(),
            repo: url.into(),
            branch: "main".into(),
        };
        push(&v, "initial").unwrap();
        assert_eq!(ahead_behind(&v).unwrap(), (0, 0));
        git(
            &t.0,
            &["clone", "--branch", "main", url, other.to_str().unwrap()],
        )
        .unwrap();
        git(&other, &["config", "user.name", "Test"]).unwrap();
        git(&other, &["config", "user.email", "test@example.invalid"]).unwrap();
        let old = commit(&other, "remote edit");
        git(&other, &["push"]).unwrap();
        fs::write(local.join("note.md"), "local edit").unwrap();
        assert!(pull(&v).is_err());
        assert!(push(&v, "blocked").is_err());
        assert_eq!(
            fs::read_to_string(local.join("note.md")).unwrap(),
            "local edit"
        );
        git(&local, &["restore", "note.md"]).unwrap();
        pull(&v).unwrap();
        assert_eq!(
            fs::read_to_string(local.join("note.md")).unwrap(),
            "remote edit"
        );
        commit(&local, "replacement");
        let backup = replace_remote(&v, &old).unwrap();
        assert_eq!(
            git(
                &remote_path,
                &["rev-parse", &format!("refs/heads/{backup}")]
            )
            .unwrap(),
            old
        );
        // Stale lease must never overwrite a newer remote branch.
        assert!(replace_remote(&v, &old).is_err());
    }
    #[test]
    fn ignores_preserve_existing_rules() {
        let t = Sandbox::new();
        fs::write(t.0.join(".gitignore"), "custom/").unwrap();
        ignore(&t.0).unwrap();
        ignore(&t.0).unwrap();
        let s = fs::read_to_string(t.0.join(".gitignore")).unwrap();
        assert!(s.starts_with("custom/\n"));
        assert_eq!(s.matches(".obsidian/").count(), 1);
    }
}
