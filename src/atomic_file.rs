use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use crate::error::{Result, YardError};

// The temporary file lives beside its destination so rename cannot cross filesystems.
// A random suffix prevents guessing; create_new also prevents following a planted link.
pub fn write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| YardError::Config(format!("invalid file path: {}", path.display())))?;
    let permissions = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(YardError::Config(format!(
                "refusing symbolic link at {}",
                path.display()
            )));
        }
        Ok(metadata) => Some(metadata.permissions()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };

    let mut random = File::open("/dev/urandom")?;
    for _ in 0..8 {
        let mut nonce = [0_u8; 16];
        random.read_exact(&mut nonce)?;
        let suffix: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
        let tmp = parent.join(format!(
            ".{}.{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("yard"),
            suffix
        ));
        let mut file = match create_temp(&tmp) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        let result = (|| -> io::Result<()> {
            file.write_all(contents)?;
            if let Some(permissions) = permissions {
                file.set_permissions(permissions)?;
            } else {
                // An explicit mode is necessary even when the umask is permissive.
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            file.sync_all()?;
            fs::rename(&tmp, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result?;
        File::open(parent)?.sync_all()?;
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "unable to allocate temporary file",
    )
    .into())
}

// Isolate allocation so the test can force a collision; production supplies random names.
fn create_temp(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn forced_temp_collision_does_not_open_a_planted_symlink() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("atomic-file-collision-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let victim = root.join("victim");
        let tmp = root.join(".target.forced.tmp");
        fs::write(&victim, "intact\n").unwrap();
        symlink(&victim, &tmp).unwrap();
        assert_eq!(
            create_temp(&tmp).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_to_string(&victim).unwrap(), "intact\n");
        assert!(fs::symlink_metadata(&tmp).unwrap().file_type().is_symlink());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_rename_cleans_its_temporary_file() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/atomic-file-rename-failure");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("directory");
        fs::create_dir_all(&path).unwrap();
        assert!(write(&path, b"test").is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn planted_fixed_temp_symlink_cannot_modify_victim() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("atomic-file-planted-temp-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("target");
        let victim = root.join("victim");
        let planted = root.join(".target.00000000000000000000000000000000.tmp");
        fs::write(&victim, "intact\n").unwrap();
        symlink(&victim, &planted).unwrap();
        write(&path, b"replacement\n").unwrap();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "intact\n");
        assert_eq!(fs::read_to_string(&path).unwrap(), "replacement\n");
        assert!(fs::symlink_metadata(&planted)
            .unwrap()
            .file_type()
            .is_symlink());
        fs::remove_dir_all(root).unwrap();
    }
}
