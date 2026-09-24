use std::fs;
use std::io;
use std::path::Path;

use crate::atomic_file;
use crate::error::{Result, YardError};

pub fn get(path: &Path, key: &str) -> Result<Option<String>> {
    let contents = fs::read_to_string(path)?;
    let prefix = format!("{key}=");
    let values: Vec<_> = contents
        .lines()
        .filter_map(|line| line.strip_prefix(&prefix))
        .collect();

    match values.as_slice() {
        [] => Ok(None),
        [value] => Ok(Some((*value).to_owned())),
        _ => Err(YardError::Config(format!(
            "{} contains duplicate {key} entries",
            path.display()
        ))),
    }
}

pub fn set(path: &Path, key: &str, value: &str) -> Result<()> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(YardError::Config(format!(
            "refusing symbolic link at {}",
            path.display()
        )));
    }
    let contents = fs::read_to_string(path)?;
    let prefix = format!("{key}=");
    let matches = contents
        .lines()
        .filter(|line| line.starts_with(&prefix))
        .count();

    if matches > 1 {
        return Err(YardError::Config(format!(
            "{} contains duplicate {key} entries",
            path.display()
        )));
    }

    let mut output = String::new();
    let mut replaced = false;
    for line in contents.lines() {
        if line.starts_with(&prefix) {
            output.push_str(&format!("{key}={value}\n"));
            replaced = true;
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }

    if !replaced {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&format!("{key}={value}\n"));
    }

    let parent = path
        .parent()
        .ok_or_else(|| YardError::Config(format!("invalid env file path: {}", path.display())))?;
    // Reject a planted legacy temporary path, rather than silently proceeding with
    // the old exploit still present. Never remove a file we did not create.
    let legacy_tmp = parent.join(format!(
        ".{}.yard.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("env")
    ));

    match fs::symlink_metadata(&legacy_tmp) {
        Ok(_) => {
            return Err(YardError::Config(format!(
                "refusing pre-existing temporary file at {}",
                legacy_tmp.display()
            )))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    atomic_file::write(path, output.as_bytes())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{get, set};

    fn temp_file() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/envfile-tests");
        fs::create_dir_all(&root).unwrap();
        root.join(format!("yard-env-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn reads_and_updates_a_value() {
        let path = temp_file();
        fs::write(&path, "FOO=one\nAPP_IMAGE_TAG=old\nBAR=two\n").unwrap();

        assert_eq!(get(&path, "APP_IMAGE_TAG").unwrap().as_deref(), Some("old"));
        set(&path, "APP_IMAGE_TAG", "new").unwrap();
        assert_eq!(get(&path, "APP_IMAGE_TAG").unwrap().as_deref(), Some("new"));

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("FOO=one\n"));
        assert!(contents.contains("APP_IMAGE_TAG=new\n"));
        assert!(contents.contains("BAR=two\n"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn appends_a_missing_value() {
        let path = temp_file();
        fs::write(&path, "FOO=one\n").unwrap();
        set(&path, "APP_IMAGE_TAG", "abc123").unwrap();
        assert_eq!(
            get(&path, "APP_IMAGE_TAG").unwrap().as_deref(),
            Some("abc123")
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn planted_regular_temp_is_rejected_and_preserved() {
        let path = temp_file();
        fs::write(&path, "APP_IMAGE_TAG=old\n").unwrap();
        let legacy = path.with_file_name(format!(
            ".{}.yard.tmp",
            path.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&legacy, "planted\n").unwrap();
        assert!(set(&path, "APP_IMAGE_TAG", "new")
            .unwrap_err()
            .to_string()
            .contains("temporary"));
        assert_eq!(fs::read_to_string(&legacy).unwrap(), "planted\n");
        assert_eq!(fs::read_to_string(&path).unwrap(), "APP_IMAGE_TAG=old\n");
        fs::remove_file(legacy).unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn target_symlink_is_rejected_without_touching_victim() {
        let path = temp_file();
        let victim = path.with_extension("victim");
        fs::write(&victim, "intact\n").unwrap();
        symlink(&victim, &path).unwrap();
        assert!(set(&path, "APP_IMAGE_TAG", "new")
            .unwrap_err()
            .to_string()
            .contains("symbolic link"));
        assert_eq!(fs::read_to_string(&victim).unwrap(), "intact\n");
        fs::remove_file(path).unwrap();
        fs::remove_file(victim).unwrap();
    }

    #[test]
    fn existing_mode_is_preserved_and_success_leaves_no_temp() {
        let path = temp_file();
        fs::write(&path, "APP_IMAGE_TAG=old\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        set(&path, "APP_IMAGE_TAG", "new").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "APP_IMAGE_TAG=new\n");
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(!fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!(".{name}."))));
        fs::remove_file(path).unwrap();
    }
}
