// Dropped files are copied into the local data inbox so the original is never
// touched and the copy survives the drag source going away.
// The inbox is swept of anything older than a week.

use std::path::{Path, PathBuf};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::{private_fs, settings};

const KEEP_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DroppedFile {
    pub name: String,
    pub path: String,
    pub size: u64,
}

pub fn inbox_dir() -> PathBuf {
    settings::local_dir().join("inbox")
}

pub fn ingest(source: &str) -> Result<DroppedFile, String> {
    private_fs::ensure_private_dir(&settings::local_dir()).map_err(|e| e.to_string())?;
    ingest_into(source, &inbox_dir())
}

fn ingest_into(source: &str, dir: &Path) -> Result<DroppedFile, String> {
    let src = Path::new(source);
    let meta = std::fs::metadata(src).map_err(|e| format!("cannot read {source}: {e}"))?;
    if !meta.is_file() {
        return Err("Only regular files can be dropped.".into());
    }

    private_fs::ensure_private_dir(dir).map_err(|e| e.to_string())?;

    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());

    let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = src.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
    let mut copy = None;
    for i in 1..1000 {
        let dest = dir.join(if i == 1 { name.clone() } else { format!("{stem} ({i}){ext}") });
        match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&dest) {
            Ok(file) => {
                copy = Some((dest, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot create attachment: {error}")),
        }
    }
    let (dest, mut file) = copy.ok_or("Too many attachments with the same name.")?;

    let copied = (|| -> std::io::Result<()> {
        let mut source = std::fs::File::open(src)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        std::io::copy(&mut source, &mut file)?;
        Ok(())
    })();
    if let Err(error) = copied {
        let _ = std::fs::remove_file(&dest);
        return Err(format!("cannot copy: {error}"));
    }
    // Age the inbox entry from when we copied it, regardless of source timestamps.
    let _ = file.set_modified(SystemTime::now());
    sweep(dir);

    Ok(DroppedFile {
        name,
        path: dest.to_string_lossy().to_string(),
        size: meta.len(),
    })
}

/// Drops anything copied here more than a week ago. `ingest` stamps every copy
/// with the time it landed, so this really is the age of the copy and not the
/// age of whatever the user happened to drag in.
fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(copied) = meta.modified() else { continue };
        if now.duration_since(copied).map(|age| age > KEEP_FOR).unwrap_or(false) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_attachment_names_never_overwrite_an_existing_copy() {
        let tmp = std::env::temp_dir().join(format!("coucou-full-inbox-{}", std::process::id()));
        let inbox = tmp.join("inbox");
        private_fs::ensure_private_dir(&inbox).unwrap();
        let source = tmp.join("note.txt");
        std::fs::write(&source, b"new").unwrap();
        for i in 1..1000 {
            let name = if i == 1 { "note.txt".into() } else { format!("note ({i}).txt") };
            std::fs::write(inbox.join(name), b"keep").unwrap();
        }

        assert_eq!(
            ingest_into(source.to_str().unwrap(), &inbox).err().as_deref(),
            Some("Too many attachments with the same name.")
        );
        assert_eq!(std::fs::read(inbox.join("note.txt")).unwrap(), b"keep");
        assert_eq!(std::fs::read(inbox.join("note (999).txt")).unwrap(), b"keep");
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn ingest_copies_and_never_overwrites() {
        let tmp = std::env::temp_dir().join(format!("coucou-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let source = tmp.join("note.txt");
        std::fs::write(&source, b"hello").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap();
        let inbox = tmp.join("inbox");

        let first = ingest_into(source.to_str().unwrap(), &inbox).unwrap();
        assert_eq!(first.name, "note.txt");
        assert_eq!(std::fs::read(&first.path).unwrap(), b"hello");
        assert_eq!(std::fs::metadata(&inbox).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&first.path).unwrap().permissions().mode() & 0o777, 0o600);

        // A second drop of the same name must not clobber the first copy.
        std::fs::write(&source, b"second").unwrap();
        let second = ingest_into(source.to_str().unwrap(), &inbox).unwrap();
        assert_ne!(first.path, second.path);
        assert_eq!(std::fs::read(&first.path).unwrap(), b"hello");
        assert_eq!(std::fs::read(&second.path).unwrap(), b"second");

        // Folders are refused rather than silently ignored.
        assert!(ingest_into(tmp.to_str().unwrap(), &inbox).is_err());

        // A pre-existing symlink cannot redirect an attachment copy.
        let target = tmp.join("target");
        std::fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, inbox.join("note (3).txt")).unwrap();
        let third = ingest_into(source.to_str().unwrap(), &inbox).unwrap();
        assert!(third.path.ends_with("note (4).txt"));
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");

        // An ancient source must not arrive already older than the sweep window.
        let old_source = tmp.join("ancient.txt");
        std::fs::write(&old_source, b"old").unwrap();
        let long_ago = SystemTime::now() - KEEP_FOR - Duration::from_secs(60 * 60);
        std::fs::File::options()
            .write(true)
            .open(&old_source)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        let aged = ingest_into(old_source.to_str().unwrap(), &inbox).unwrap();
        assert!(
            Path::new(&aged.path).exists(),
            "a file copied just now was swept as if it were a week old"
        );
        let _ = std::fs::remove_file(&aged.path);

        let _ = std::fs::remove_file(&first.path);
        let _ = std::fs::remove_file(&second.path);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
