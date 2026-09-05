use std::fs;
use std::path::Path;

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
    let metadata = fs::metadata(path)?;
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
    let tmp = parent.join(format!(
        ".{}.yard.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("env")
    ));

    fs::write(&tmp, output)?;
    fs::set_permissions(&tmp, metadata.permissions())?;
    fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{get, set};

    fn temp_file() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("yard-env-{}-{nonce}", std::process::id()))
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
}
