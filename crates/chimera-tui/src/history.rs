//! Private, paged scrollback. The live renderer stays bounded independently of history.
use crate::message::Message;
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

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
        let end = self.count.saturating_sub(page.saturating_mul(size));
        let start = end.saturating_sub(size);
        BufReader::new(File::open(&self.path)?)
            .lines()
            .skip(start)
            .take(end - start)
            .map(|line| Ok(serde_json::from_str(&line?)?))
            .collect()
    }
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Message>> {
        let query = query.to_lowercase();
        let mut found = VecDeque::new();
        for line in BufReader::new(File::open(&self.path)?).lines() {
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
