# obvault

A Rust CLI for keeping a local Obsidian vault in a GitHub repository, with explicit push/pull and recoverable version restoration. Independent community project; not affiliated with Obsidian.

## Install globally

Install [Rust](https://rustup.rs/) and Git, then from this source directory:

```sh
cargo install --path . --locked
obvault --help
```

Cargo installs to `~/.cargo/bin` (Windows: `%USERPROFILE%\.cargo\bin`). Put that directory on your PATH to run `obvault` anywhere. Source supports macOS, Windows and Linux; local validation was performed on macOS. Git must be on PATH. Configure your Git name/email and GitHub authentication using your existing SSH setup, credential manager, or `gh auth setup-git`. Obvault does not store credentials. This package is not yet published to crates.io.

## Setup

```sh
obvault setup
obvault setup --vault work
```

Select a detected Obsidian vault or enter a path. Accepts spaces, relative paths and `~`. Supply a GitHub HTTPS or SSH URL and a branch. Workflows:

1. **Upload local vault:** initialize Git when necessary, add ignore rules, commit and upload. If the remote branch cannot be advanced normally, setup explicitly asks before replacing it. It first creates and verifies `obvault-backup-<timestamp>` on GitHub, then uses a lease-protected push. Backup failure or a changed remote branch prevents replacement. Any backup already created is retained.
2. **Download:** clone an existing remote branch into a new or empty destination.
3. **Connect existing:** save configuration for an existing checkout without syncing. Changing origin requires confirmation.

Run setup again to update values; defaults show existing configuration. Choose workflow 3 for configuration-only updates. The first configured vault is the default; select others using `--vault NAME` on any command.

Configuration lives in `~/Library/Application Support/obvault/config.json` on macOS, `$XDG_CONFIG_HOME/obvault/config.json` (or `~/.config/...`) on Linux, and `%APPDATA%\obvault\config.json` on Windows. `OBVAULT_CONFIG` overrides the file location.

## Commands

```sh
obvault status
obvault diff
obvault diff --remote
obvault push -m "Update project notes"
obvault pull
obvault history
obvault revert COMMIT
obvault revert COMMIT --file "Projects/My note.md"
obvault open
```

- `status` fetches, reports uncommitted changes and ahead/behind counts; offline checks report remote status as unknown.
- `diff` compares tracked local files with HEAD, and lists untracked filenames separately. `--remote` fetches and compares tracked working files with the configured remote branch. Ignored files are excluded.
- `push` fetches and checks remote ancestry before staging anything. It commits **all nonignored local edits, additions and deletions**, then pushes normally. If upload fails after committing, that local commit remains available for retry. Default messages contain a timestamp.
- `pull` requires a clean working tree and only fast-forwards. Diverged histories require resolution using Git; obvault never resolves conflicts automatically.
- `history` shows the latest 30 local commits, including downloaded versions after a pull.
- `revert` restores the selected commit's snapshot, rather than undoing one commit's patch. It shows a preview and requires confirmation, then creates a new local commit. Whole-vault restore can remove tracked notes added after the selected version. Untracked/ignored files are not restored; ignored files are not backups. Use `push` afterward to upload the restoration.
- `open` invokes the Obsidian URI handler. It does not sync automatically.

## Privacy and limitations

Create a **private vault repository** for personal notes. Keep this public CLI source separate from vault contents. Obvault does not create GitHub repositories or change their visibility.

Upload setup ignores `.obsidian/`, `.trash/`, `.DS_Store`, `.env`, `.env.*`, `*.pem`, and `*.key`. Existing ignore rules are preserved. Already tracked files remain tracked. These rules are not secret scanning: review notes and attachments before uploading. `.obsidian` exclusion means plugin settings and themes are not synchronized.

Use one obvault/Git operation per vault at a time. Avoid editing notes during pull/restoration. Git hooks run according to your existing Git configuration. Standard GitHub file-size limits apply; Git LFS is not configured automatically. Empty folders are not stored by Git. A separate device backup is still recommended.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Tests use temporary local repositories, never your vault or GitHub account. CI checks macOS, Windows and Linux. MIT licensed.
