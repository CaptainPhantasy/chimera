//! Read-only change review and exact-action approval previews.
//!
//! Previewing never grants permission or writes project files. Decisions carry
//! the pending action UUID back to the runner, which owns the original call.
use crate::dialog::{Choice, Dialog};
use anyhow::{Context, Result};
use chimera_core::approval::display_value;
use chimera_tools::ToolCall;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use uuid::Uuid;

const FILE_BYTES: usize = 2 * 1024 * 1024;
const REVIEW_BYTES: usize = 16 * 1024 * 1024;

/// Produce an inspectable before/after preview, with refusal selected first.
/// File reads are bounded; the runner may call this in a blocking worker.
pub fn approval(root: &Path, call: &ToolCall, id: Uuid) -> Dialog {
    let mut actions = vec![Choice::new(
        "Deny this action",
        "Refuse the pending action; no project change is approved.",
        format!("approval:deny:{id}"),
    )];
    let preview = approval_text(root, call);
    let text = match preview {
        Ok(text) => {
            actions.push(Choice::new(
                "Approve once",
                "Allow only this exact pending action once.",
                format!("approval:approve:{id}"),
            ));
            text
        }
        Err(error) => format!(
            "Cannot safely preview this action: {error:#}\n\nApproval is unavailable in this preview. Deny it and ask for a corrected action."
        ),
    };
    Dialog::text(
        &format!("Review action: {}", call.tool),
        "Review the full change below. Tab selects a decision; Enter confirms. Esc leaves the action waiting. Credential-looking lines are masked; the actual pending action is unchanged.",
        text,
        actions,
    )
}

fn approval_text(root: &Path, call: &ToolCall) -> Result<String> {
    let shown = display_value(call.args.clone());
    let mut text = format!("Tool: {}\n", call.tool);
    if let Some(world) = call.world {
        // A selected execution world can have a different root. A host-file
        // comparison would misrepresent what that action is about to change.
        anyhow::ensure!(
            !matches!(call.tool.as_str(), "write_file" | "edit_file"),
            "file action belongs to execution world {world}; review it in that world's working folder"
        );
    }
    match call.tool.as_str() {
        "write_file" | "edit_file" => {
            let name = call.args["path"].as_str().context("missing file path")?;
            let path = safe_path(root, name)?;
            let before = match std::fs::symlink_metadata(&path) {
                Ok(_) => read_text(&path)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(error) => return Err(error.into()),
            };
            let after = if call.tool == "write_file" {
                call.args["content"]
                    .as_str()
                    .context("missing file content")?
                    .to_string()
            } else {
                let old = call.args["old_text"].as_str().context("missing old_text")?;
                let new = call.args["new_text"].as_str().context("missing new_text")?;
                anyhow::ensure!(
                    !old.is_empty() && before.matches(old).count() == 1,
                    "the existing text must match exactly once; ask the agent to read the file again"
                );
                before.replacen(old, new, 1)
            };
            anyhow::ensure!(
                after.len() <= FILE_BYTES,
                "proposed file exceeds the 2 MiB preview limit"
            );
            text.push_str(&format!("File: {name}\n\n"));
            text.push_str(&patch(name, &before, &after));
            text.push_str("\nThis is a preview of the current file. Existing execution-time conflict and exact-action checks still apply.\n");
        }
        _ => {
            text.push_str("\nExact action arguments (configured secrets masked):\n");
            text.push_str(&serde_json::to_string_pretty(&shown)?);
        }
    }
    Ok(mask_credentials(&text))
}

fn private_component(value: &str) -> bool {
    matches!(
        value,
        ".git" | ".chimera" | ".ssh" | ".aws" | ".codex" | ".supercache" | ".agents"
    ) || value == ".env"
        || (value.starts_with(".env.") && value != ".env.example")
}

fn safe_path(root: &Path, value: &str) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .context("project folder is unavailable")?;
    let raw = Path::new(value);
    anyhow::ensure!(
        !raw.components()
            .any(|part| matches!(part, Component::ParentDir)),
        "parent traversal is not allowed"
    );
    let candidate = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        root.join(raw)
    };
    let relative = candidate
        .strip_prefix(&root)
        .context("file is outside this project")?;
    anyhow::ensure!(
        !relative.as_os_str().is_empty(),
        "choose a file, not the project folder"
    );
    let mut checked = root;
    for part in relative.components() {
        anyhow::ensure!(
            !private_component(&part.as_os_str().to_string_lossy()),
            "protected files cannot be previewed"
        );
        checked.push(part);
        match std::fs::symlink_metadata(&checked) {
            Ok(meta) => anyhow::ensure!(
                !meta.file_type().is_symlink(),
                "symbolic links cannot be previewed"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(checked)
}

fn read_text(path: &Path) -> Result<String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "macos")]
        options.custom_flags(0x100); // O_NOFOLLOW
        #[cfg(not(target_os = "macos"))]
        options.custom_flags(0x20000);
    }
    let file = options
        .open(path)
        .context("cannot read the existing file")?;
    anyhow::ensure!(file.metadata()?.is_file(), "path is not a regular file");
    let mut bytes = Vec::new();
    file.take((FILE_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= FILE_BYTES,
        "existing file exceeds the 2 MiB preview limit"
    );
    String::from_utf8(bytes).context("binary files cannot be previewed as a text change")
}

/// A single exact replacement hunk with surrounding context. Unlike a visual
/// line cap, it preserves every changed line, including edits beyond line 400.
fn patch(name: &str, before: &str, after: &str) -> String {
    if before == after {
        return "No content change.\n".into();
    }
    let old: Vec<&str> = before.split_inclusive('\n').collect();
    let new: Vec<&str> = after.split_inclusive('\n').collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let start = prefix.saturating_sub(3);
    let old_end = (old.len() - suffix + 3).min(old.len());
    let new_end = (new.len() - suffix + 3).min(new.len());
    let mut result = format!(
        "--- a/{name}\n+++ b/{name}\n@@ -{},{} +{},{} @@\n",
        if old.is_empty() { 0 } else { start + 1 },
        old_end - start,
        if new.is_empty() { 0 } else { start + 1 },
        new_end - start
    );
    for line in &old[start..prefix] {
        push_patch_line(&mut result, ' ', line);
    }
    for line in &old[prefix..old.len() - suffix] {
        push_patch_line(&mut result, '-', line);
    }
    for line in &new[prefix..new.len() - suffix] {
        push_patch_line(&mut result, '+', line);
    }
    for line in &old[old.len() - suffix..old_end] {
        push_patch_line(&mut result, ' ', line);
    }
    result
}

fn push_patch_line(output: &mut String, prefix: char, line: &str) {
    output.push(prefix);
    output.push_str(line);
    if !line.ends_with('\n') {
        output.push_str("\n\\ No newline at end of file\n");
    }
}

fn mask_credentials(text: &str) -> String {
    let mut private_key = false;
    text.lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            let key_start = lower.contains("-----begin ") && lower.contains("private key-----");
            let key_end = lower.contains("-----end ") && lower.contains("private key-----");
            let credential = private_key
                || key_start
                || [
                    "api_key",
                    "apikey",
                    "password",
                    "authorization",
                    "access_token",
                    "client_secret",
                ]
                .iter()
                .any(|key| lower.contains(key) && (line.contains('=') || line.contains(':')))
                || ["sk-", "ghp_", "Bearer "]
                    .iter()
                    .any(|prefix| line.contains(prefix));
            private_key = (private_key || key_start) && !key_end;
            if credential {
                let marker = line
                    .chars()
                    .next()
                    .filter(|c| matches!(c, '+' | '-' | ' '))
                    .unwrap_or(' ');
                format!("{marker}[credential-looking line masked]")
            } else {
                // File content must not inject terminal-control sequences into
                // the review panel. Newline boundaries are retained by join.
                line.chars()
                    .filter(|c| !c.is_control() || *c == '\t')
                    .collect()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A nonblocking, aggregate review of the selected project's Git changes.
#[derive(Default)]
pub struct GitReview {
    root: Option<PathBuf>,
    job: Option<tokio::task::JoinHandle<Result<String>>>,
}

impl GitReview {
    pub fn open(&mut self, root: PathBuf) -> Dialog {
        self.cancel();
        self.root = Some(root.clone());
        self.job = Some(tokio::spawn(async move { changes(&root).await }));
        Dialog::busy(
            "Project changes",
            "Reading staged, unstaged and untracked changes. No files are changed.",
        )
    }

    pub fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.abort();
        }
        self.root = None;
    }

    pub async fn action(&mut self, action: &str, dialog: &mut Option<Dialog>) -> Result<()> {
        if action == "review:refresh" {
            let root = self
                .root
                .clone()
                .context("Open /diff before refreshing the review")?;
            *dialog = Some(self.open(root));
        }
        Ok(())
    }

    pub async fn poll(&mut self, dialog: &mut Option<Dialog>) {
        if !self.job.as_ref().is_some_and(|job| job.is_finished()) {
            return;
        }
        let Some(job) = self.job.take() else { return };
        let result = job
            .await
            .unwrap_or_else(|error| Err(anyhow::anyhow!("Change review stopped: {error}")));
        let text = match result {
            Ok(text) => text,
            Err(error) => format!("Could not read project changes: {error:#}\n\nCheck that the project is a readable Git working tree, then choose Refresh. Your files are unchanged."),
        };
        *dialog = Some(Dialog::text("Project changes", "Read-only snapshot · ↑↓/Page keys scroll · Tab selects Refresh · Esc returns to chat. Credential-looking lines and protected files are omitted.", text,
            vec![Choice::new("Refresh", "Read the current working tree again", "review:refresh")]));
    }
}

impl Drop for GitReview {
    fn drop(&mut self) {
        self.cancel();
    }
}

async fn changes(root: &Path) -> Result<String> {
    let root = root.canonicalize()?;
    let status = git(&root, &["status", "--short", "--untracked-files=normal"]).await?;
    let paths = [
        "--",
        ".",
        ":(exclude).env",
        ":(exclude).env.*",
        ":(exclude).chimera/**",
        ":(exclude).ssh/**",
        ":(exclude).aws/**",
        ":(exclude).codex/**",
        ":(exclude).agents/**",
        ":(exclude).supercache/**",
        ":(exclude)**/.env",
        ":(exclude)**/.env.*",
        ":(exclude)**/.chimera/**",
        ":(exclude)**/.ssh/**",
        ":(exclude)**/.aws/**",
        ":(exclude)**/.codex/**",
        ":(exclude)**/.agents/**",
        ":(exclude)**/.supercache/**",
    ];
    let mut staged = vec![
        "diff",
        "--cached",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
    ];
    staged.extend(paths);
    let mut unstaged = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-color"];
    unstaged.extend(paths);
    let staged = git(&root, &staged).await?;
    let unstaged = git(&root, &unstaged).await?;
    Ok(mask_credentials(&format!("Project: {}\n\nCHANGED FILES (index / working tree)\n{}\nSTAGED PATCH\n{}\nUNSTAGED PATCH\n{}\nUntracked files are marked ?? above; their contents are not in a Git patch until added. Protected-file contents are excluded. This review does not stage, discard or commit anything.\n",
        root.display(), if status.is_empty() { "Working tree clean.\n" } else { &status }, if staged.is_empty() { "No staged changes.\n" } else { &staged }, if unstaged.is_empty() { "No unstaged changes.\n" } else { &unstaged })))
}

async fn git(root: &Path, args: &[&str]) -> Result<String> {
    let mut command = tokio::process::Command::new("git");
    command
        .current_dir(root)
        .args([
            "--no-optional-locks",
            "--no-pager",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.quotePath=true",
            "-c",
            "diff.external=",
        ])
        .arg("-c")
        .arg(format!("safe.directory={}", root.display()))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Do not inherit command/config injection from a parent Git invocation.
    for (key, _) in std::env::vars_os().filter(|(key, _)| key.to_string_lossy().starts_with("GIT_"))
    {
        command.env_remove(key);
    }
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    let mut child = command.spawn().context("Git could not be started")?;
    let stdout = child.stdout.take().context("Git output is unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("Git error output is unavailable")?;
    let read = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut stdout = stdout.take((REVIEW_BYTES + 1) as u64);
        let mut stderr = stderr.take(65_537);
        let (out_read, err_read) =
            tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
        out_read?;
        err_read?;
        anyhow::ensure!(out.len() <= REVIEW_BYTES && err.len() <= 65_536, "change review exceeds the 16 MiB display limit; narrow the working changes before reviewing again");
        let status = child.wait().await?;
        anyhow::ensure!(status.success(), "{}", String::from_utf8_lossy(&err).trim());
        Ok::<_, anyhow::Error>(String::from_utf8_lossy(&out).into_owned())
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), read)
        .await
        .context("Git review timed out; cancel or retry the review")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog::{Body, Outcome};
    use crate::theme::Styles;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{backend::TestBackend, Terminal};

    fn temp_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("chimera-review-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn ux_03_approval_shows_the_real_file_change_and_defaults_to_deny() {
        // WHAT: read an approval like a developer and press the default action.
        // WHY: edits must be understandable before granting exact permission.
        // PASS: the screen shows old/new text, Enter denies, the file is unchanged.
        let root = temp_root();
        std::fs::write(root.join("answer.rs"), "fn old_name() {}\n").unwrap();
        let id = Uuid::new_v4();
        let call = ToolCall {
            tool: "edit_file".into(),
            args: serde_json::json!({"path":"answer.rs","old_text":"old_name","new_text":"new_name"}),
            world: None,
            requested_by: Uuid::new_v4(),
        };
        let original = serde_json::to_value(&call).unwrap();
        let mut dialog = approval(&root, &call, id);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| dialog.render(frame, &Styles::dark()))
            .unwrap();
        let visible: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(visible.contains("answer.rs") && visible.contains("-fn old_name() {}") && visible.contains("+fn new_name() {}") && matches!(dialog.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Outcome::Action(action, _) if action == format!("approval:deny:{id}")) && std::fs::read_to_string(root.join("answer.rs")).unwrap() == "fn old_name() {}\n" && serde_json::to_value(&call).unwrap() == original,
            "The developer must see the actual edit and default to refusing it without changing files or the pending action: {visible}");
    }

    #[tokio::test]
    async fn ux_04_diff_reviews_staged_unstaged_and_untracked_work_without_changing_it() {
        // WHAT: open /diff over a real Git fixture containing all change states.
        // WHY: a developer needs one reliable review before accepting a result.
        // PASS: the panel contains both patches and the untracked filename while
        // the working file and index remain exactly as the developer left them.
        let root = temp_root();
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-c")
                .arg(format!("safe.directory={}", root.display()))
                .current_dir(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        run(&["init", "--quiet"]);
        std::fs::write(root.join("work.txt"), "staged version\n").unwrap();
        run(&["add", "--", "work.txt"]);
        std::fs::write(root.join("work.txt"), "working version\n").unwrap();
        std::fs::write(root.join("new.txt"), "not staged\n").unwrap();
        std::fs::write(root.join(".env"), "PRIVATE_CONTENT_SENTINEL\n").unwrap();
        run(&["add", "--", ".env"]);
        let index_before = run(&["show", ":work.txt"]);
        let mut review = GitReview::default();
        let mut dialog = Some(review.open(root.clone()));
        while review.job.is_some() {
            review.poll(&mut dialog).await;
            tokio::task::yield_now().await;
        }
        let text = match &dialog.as_ref().unwrap().body {
            Body::Text { text, .. } => text,
            _ => panic!("developer did not receive a scrollable review"),
        };
        assert!(text.contains("STAGED PATCH") && text.contains("+staged version") && text.contains("UNSTAGED PATCH") && text.contains("+working version") && text.contains("?? new.txt") && !text.contains("PRIVATE_CONTENT_SENTINEL") && std::fs::read_to_string(root.join("work.txt")).unwrap() == "working version\n" && run(&["show", ":work.txt"]) == index_before,
            "The complete review must show both change layers and untracked files without modifying either the working tree or index: {text}");
    }
}
