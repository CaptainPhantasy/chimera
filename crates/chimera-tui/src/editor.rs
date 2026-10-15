//! chimera-tui::editor — Compose long prompts in an external editor.
//!
//! `/editor` (or Ctrl+E) writes the unsent draft to a private, uniquely
//! named temporary file, hands the terminal to the configured editor, and
//! restores the chat screen exactly when the editor exits. The saved text
//! then replaces the draft — nothing is submitted automatically; the text
//! waits in the input until Enter is pressed.
//!
//! The configured editor may include arguments (`"code -w"`); the draft file
//! path is appended as the final argument. Quotes are not interpreted. When
//! no editor is configured, or the editor cannot start, exits non-zero, or
//! saves nothing usable, the chat screen still comes back and the original
//! draft is preserved.

use crate::message::SystemKind;
use crate::runner::{init_terminal, restore_terminal, TuiTerminal};
use anyhow::{anyhow, Context, Result};
use chimera_config::Config;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Mirrors the input bar's own cap so an oversized edit is refused honestly
/// instead of producing a draft the input can no longer edit.
const MAX_DRAFT_BYTES: usize = 65_536;

/// What one editor round trip decided.
#[derive(Debug)]
pub struct EditorOutcome {
    /// The replacement draft; absent when the input keeps what it had.
    pub draft: Option<String>,
    /// The user-facing summary of what happened.
    pub message: String,
    pub kind: SystemKind,
}

impl EditorOutcome {
    fn unchanged(message: String) -> Self {
        Self {
            draft: None,
            message,
            kind: SystemKind::Info,
        }
    }

    fn replaced(draft: String) -> Self {
        Self {
            draft: Some(draft),
            message: "Draft updated from the editor. Nothing is sent until you press Enter.".into(),
            kind: SystemKind::Success,
        }
    }
}

/// Suspend the chat screen, run the configured editor on `draft`, and bring
/// the screen back. `terminal` is replaced with a freshly initialized one so
/// drawing continues normally afterwards. An `Err` return means the original
/// draft is unchanged; even then the screen is restored before reporting.
pub fn suspend_and_edit(
    terminal: &mut TuiTerminal,
    config: &Config,
    draft: &str,
) -> Result<EditorOutcome> {
    restore_terminal().context("could not hand the terminal to the editor")?;
    let outcome = edit_draft(config, draft);
    // The chat screen comes back even when the editor failed. A failure here
    // means the terminal itself is gone; it propagates to the caller.
    *terminal = init_terminal().context("could not restore the chat screen after the editor")?;
    outcome
}

/// Run one editor round trip without touching the terminal: write `draft` to
/// a private temporary file, run the configured editor on it, and read the
/// result back. Every failure keeps the original draft.
pub fn edit_draft(config: &Config, draft: &str) -> Result<EditorOutcome> {
    edit_draft_in(&std::env::temp_dir(), config, draft)
}

/// [`edit_draft`] with an injectable temporary directory for tests.
fn edit_draft_in(directory: &Path, config: &Config, draft: &str) -> Result<EditorOutcome> {
    let command = config.editor().context(
        "No editor is configured. Set \"editor\" in your settings file, or the VISUAL or EDITOR environment variable.",
    )?;
    let path = write_draft(directory, draft)?;
    let outcome = run_editor_and_read_back(&command, &path, draft);
    // The temporary draft is removed whatever happened; a leftover file is
    // reported, never abandoned silently.
    match std::fs::remove_file(&path) {
        Ok(()) => outcome,
        Err(error) => outcome.map(|mut outcome| {
            outcome.message.push_str(&format!(
                " The temporary draft file could not be removed: {error}"
            ));
            outcome
        }),
    }
}

/// Write the draft to a uniquely named file only this user can read.
fn write_draft(directory: &Path, draft: &str) -> Result<PathBuf> {
    let path = directory.join(format!(
        "chimera-draft-{}-{}.md",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("could not create the draft file {}", path.display()))?;
    file.write_all(draft.as_bytes())
        .with_context(|| format!("could not write the draft file {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("could not save the draft file {}", path.display()))?;
    Ok(path)
}

/// Run the configured editor and turn its result into the next draft.
fn run_editor_and_read_back(command: &str, path: &Path, original: &str) -> Result<EditorOutcome> {
    let mut parts = command.split_whitespace();
    let program = parts.next().context("The configured editor is empty.")?;
    let status = std::process::Command::new(program)
        .args(parts)
        .arg(path)
        .status()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => anyhow!(
                "Could not start the editor \"{program}\": program not found. Your draft is unchanged."
            ),
            _ => anyhow!(
                "Could not start the editor \"{program}\": {error}. Your draft is unchanged."
            ),
        })?;
    if !status.success() {
        return Err(match status.code() {
            Some(code) => anyhow!(
                "The editor \"{program}\" exited with status {code}. Your draft is unchanged."
            ),
            None => anyhow!(
                "The editor \"{program}\" was terminated before finishing. Your draft is unchanged."
            ),
        });
    }
    let edited = std::fs::read_to_string(path).map_err(|error| {
        anyhow!("Could not read the edited draft back: {error}. Your draft is unchanged.")
    })?;
    if edited.is_empty() {
        return Ok(EditorOutcome::unchanged(
            "The editor saved an empty file. Your draft is unchanged.".into(),
        ));
    }
    if edited == original {
        return Ok(EditorOutcome::unchanged(
            "The editor closed without changes. Your draft is as it was.".into(),
        ));
    }
    if edited.len() > MAX_DRAFT_BYTES {
        return Err(anyhow!(
            "The edited text is {} bytes; the input keeps at most {MAX_DRAFT_BYTES}. Your previous draft is unchanged.",
            edited.len()
        ));
    }
    Ok(EditorOutcome::replaced(edited))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_dir() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "chimera-editor-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn draft_files(dir: &Path) -> Vec<String> {
        dir.read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("chimera-draft-"))
            .collect()
    }

    /// A shell script standing in for a real editor: it transforms the draft
    /// file and never reads the terminal.
    #[cfg(unix)]
    fn script_editor(directory: &Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn config_with_editor(editor: &str) -> Config {
        Config {
            editor: editor.into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_draft_file_is_private_unique_and_holds_the_draft() {
        let dir = temp_dir();
        let first = write_draft(&dir, "secret draft 世界").unwrap();
        let second = write_draft(&dir, "secret draft 世界").unwrap();
        assert_ne!(first, second, "draft files must be uniquely named");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&first).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the draft file must be private");
        }
        assert_eq!(
            std::fs::read_to_string(&first).unwrap(),
            "secret draft 世界"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_edited_file_replaces_the_draft_and_the_temp_file_is_cleaned_up() {
        let dir = temp_dir();
        #[cfg(unix)]
        {
            let editor = script_editor(
                &dir,
                "append.sh",
                "printf 'edited: ' | cat - \"$1\" > \"$1.new\" && mv \"$1.new\" \"$1\"",
            );
            let outcome = edit_draft_in(&dir, &config_with_editor(&editor), "hello 世界").unwrap();
            assert_eq!(outcome.draft.as_deref(), Some("edited: hello 世界"));
            assert!(outcome.message.contains("Nothing is sent"));
        }
        #[cfg(not(unix))]
        {
            let _ = edit_draft_in(&dir, &config_with_editor("no-such-editor"), "hello");
        }
        assert!(draft_files(&dir).is_empty(), "temp draft must be removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_large_multibyte_draft_survives_the_round_trip() {
        let dir = temp_dir();
        let editor = script_editor(
            &dir,
            "suffix.sh",
            "printf '世界 café 🦋 end' | cat - \"$1\" > \"$1.new\" && mv \"$1.new\" \"$1\"",
        );
        let draft = "世界 café 🦋 line\n".repeat(400);
        let outcome = edit_draft_in(&dir, &config_with_editor(&editor), &draft).unwrap();
        let mut expected = "世界 café 🦋 end".to_string();
        expected.push_str(&draft);
        assert_eq!(outcome.draft.as_deref(), Some(expected.as_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_editor_reports_an_error_and_keeps_the_draft() {
        let dir = temp_dir();
        let error = edit_draft_in(
            &dir,
            &config_with_editor("chimera-no-such-editor-xyz"),
            "keep me",
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("program not found"), "{message}");
        assert!(message.contains("unchanged"), "{message}");
        assert!(draft_files(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn an_editor_that_exits_non_zero_keeps_the_draft() {
        let dir = temp_dir();
        let editor = script_editor(&dir, "failing.sh", "exit 3");
        let error = edit_draft_in(&dir, &config_with_editor(&editor), "keep me").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("exited with status 3"), "{message}");
        assert!(message.contains("unchanged"), "{message}");
        assert!(draft_files(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn an_empty_save_keeps_the_draft() {
        let dir = temp_dir();
        let editor = script_editor(&dir, "empty.sh", ": > \"$1\"");
        let outcome = edit_draft_in(&dir, &config_with_editor(&editor), "keep me").unwrap();
        assert_eq!(outcome.draft, None);
        assert!(
            outcome.message.contains("empty file"),
            "{}",
            outcome.message
        );
        assert!(draft_files(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn closing_the_editor_without_changes_reports_it() {
        let dir = temp_dir();
        let editor = script_editor(&dir, "noop.sh", "exit 0");
        let outcome = edit_draft_in(&dir, &config_with_editor(&editor), "same text").unwrap();
        assert_eq!(outcome.draft, None);
        assert!(
            outcome.message.contains("without changes"),
            "{}",
            outcome.message
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn an_oversized_edit_is_refused_with_the_draft_kept() {
        let dir = temp_dir();
        let editor = script_editor(
            &dir,
            "huge.sh",
            // 140000 bytes from `yes` halve to 70000 after newlines go.
            "yes x | head -c 140000 | tr -d '\\n' > \"$1\"",
        );
        let error = edit_draft_in(&dir, &config_with_editor(&editor), "keep me").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("70000 bytes"), "{message}");
        assert!(message.contains("65536"), "{message}");
        assert!(message.contains("unchanged"), "{message}");
        assert!(draft_files(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_editor_configured_names_the_fix() {
        let dir = temp_dir();
        let error = edit_draft_in(&dir, &Config::default(), "draft").unwrap_err();
        assert!(error.to_string().contains("No editor is configured"));
        assert!(draft_files(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
