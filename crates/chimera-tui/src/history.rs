//! Private, paged scrollback. The live renderer stays bounded independently of history.
use crate::message::Message;
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// A stable range ending before an absolute archive offset.
pub struct Page {
    pub messages: Vec<Message>,
    pub before: usize,
}

#[derive(Debug, Clone)]
pub struct Archive {
    path: PathBuf,
    pub count: usize,
}

impl Archive {
    pub fn create(root: &Path) -> Result<Self> {
        let base = root.join(".chimera");
        for directory in [&base, &base.join("scrollback")] {
            if let Ok(meta) = directory.symlink_metadata() {
                anyhow::ensure!(
                    !meta.file_type().is_symlink() && meta.is_dir(),
                    "scrollback directory is unsafe"
                );
            } else {
                std::fs::create_dir(directory)?;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        let path = base
            .join("scrollback")
            .join(format!("{}.jsonl", uuid::Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&path)?;
        Ok(Self { path, count: 0 })
    }
    pub fn append(&mut self, message: &Message) -> Result<()> {
        let mut file = OpenOptions::new().append(true).open(&self.path)?;
        anyhow::ensure!(file.metadata()?.len() < 128 * 1024 * 1024, "this view's scrollback reached 128 MiB; reopen the saved session to inspect its records");
        let mut line = serde_json::to_vec(message)?;
        line.push(b'\n');
        let previous_len = file.metadata()?.len();
        if let Err(error) = file.write_all(&line) {
            // A failed append must not poison every later page with a partial JSON record.
            file.set_len(previous_len)
                .context("could not recover interrupted scrollback append")?;
            return Err(error).context("could not preserve older screen output");
        }
        self.count += 1;
        Ok(())
    }
    /// A page counted backwards from the end; does not allocate the full archive.
    pub fn page(&self, page: usize, size: usize) -> Result<Vec<Message>> {
        self.page_before(self.count.saturating_sub(page.saturating_mul(size)), size)
            .map(|page| page.messages)
    }

    /// Read older messages using the previous page's absolute `before` cursor.
    /// Later appends cannot shift this range or duplicate already viewed rows.
    pub fn page_before(&self, before: usize, size: usize) -> Result<Page> {
        let end = before.min(self.count);
        let start = end.saturating_sub(size);
        let messages = BufReader::new(File::open(&self.path)?)
            .lines()
            .skip(start)
            .take(end - start)
            .map(|line| Ok(serde_json::from_str(&line?)?))
            .collect::<Result<Vec<_>>>()?;
        Ok(Page {
            messages,
            before: start,
        })
    }
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Message>> {
        let query = query.to_lowercase();
        let mut found = VecDeque::new();
        for line in BufReader::new(File::open(&self.path)?)
            .lines()
            .take(self.count)
        {
            let message: Message = serde_json::from_str(&line?)?;
            if message.plain_text().to_lowercase().contains(&query) {
                found.push_back(message);
                if found.len() > limit {
                    found.pop_front();
                }
            }
        }
        Ok(found.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WHAT: paging older output retains the original boundary while output arrives.
    /// WHY: an engineer must not miss or reread messages during a running command.
    #[test]
    fn reviewfix_05_history_cursor_preserves_contiguous_pages_during_append() {
        let root =
            std::env::temp_dir().join(format!("chimera-history-review-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let mut archive = Archive::create(&root).unwrap();
        for n in 0..6 {
            archive
                .append(&Message::User {
                    text: n.to_string(),
                })
                .unwrap();
        }
        let latest = archive.page_before(archive.count, 2).unwrap();
        archive
            .append(&Message::User {
                text: "new output".into(),
            })
            .unwrap();
        let middle = archive.page_before(latest.before, 2).unwrap();
        let earliest = archive.page_before(middle.before, 2).unwrap();
        let visible: Vec<_> = earliest
            .messages
            .iter()
            .chain(&middle.messages)
            .chain(&latest.messages)
            .map(Message::plain_text)
            .collect();
        assert_eq!(
            visible,
            ["0", "1", "2", "3", "4", "5"],
            "PASS only when all original output appears once, without gaps or overlap"
        );
        assert_eq!(earliest.before, 0);
        std::fs::remove_dir_all(root).unwrap();
    }
}
