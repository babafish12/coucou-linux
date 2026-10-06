// App-owned settings, logs and attachments are readable only by their owner.
// Based on upstream's Linux file-permission hardening (bef6558).

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

/// Preserve an existing file (including a settings symlink), while closing
/// permissions left behind by older releases. Parent directories are prepared
/// by the caller; only app-owned files belong here.
pub fn open_private_file(path: &Path, append: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .append(append)
        .truncate(!append)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn private_permissions_cover_new_and_existing_data() {
        let root = std::env::temp_dir().join(format!("coucou-private-fs-{}", std::process::id()));
        ensure_private_dir(&root).unwrap();
        let dir = root.join("data");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&dir).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);

        let path = dir.join("preferences.json");
        open_private_file(&path, false).unwrap().write_all(b"first").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        open_private_file(&path, true).unwrap().write_all(b" second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first second");
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

        let link = dir.join("settings.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        open_private_file(&link, false).unwrap().write_all(b"updated").unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&path).unwrap(), b"updated");
        fs::remove_dir_all(root).unwrap();
    }
}
