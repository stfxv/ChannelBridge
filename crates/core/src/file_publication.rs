use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) enum Publication {
    Settings,
    NewReport,
}

struct StagingPath {
    path: PathBuf,
    remove_on_drop: bool,
}

impl Drop for StagingPath {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl Publication {
    pub(super) fn write(self, destination: &Path, data: &[u8]) -> io::Result<()> {
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let label = match self {
            Self::Settings => {
                fs::create_dir_all(parent)?;
                "settings"
            }
            Self::NewReport => "diagnostic",
        };
        // Only one of the two basenames contains a hyphen, even on a
        // case-insensitive volume. The staging name cannot be the destination.
        let separator = if destination
            .file_name()
            .is_some_and(|name| name.as_encoded_bytes().contains(&b'-'))
        {
            '_'
        } else {
            '-'
        };
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let (mut file, mut staging) = loop {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let path = parent.join(format!(
                ".channelbridge{separator}{label}{separator}{pid}{separator}{sequence}.tmp"
            ));
            match options.open(&path) {
                Ok(file) => {
                    break (
                        file,
                        StagingPath {
                            path,
                            remove_on_drop: true,
                        },
                    );
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        match self {
            Self::Settings => {
                fs::rename(&staging.path, destination)?;
                staging.remove_on_drop = false;
            }
            Self::NewReport => fs::hard_link(&staging.path, destination)?,
        }
        drop(staging);
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    }
}
