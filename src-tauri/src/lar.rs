//! User-visible data root (`~/.lar`) and one-time legacy → Lar copy.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const GHOSTNOTE_BUNDLE: &str = "com.ghostnote.app";
const MARKER: &str = ".migrated-from-legacy";
const PRIOR_CODA_DIR: &str = ".coda";

/// `LAR_HOME` if set, else `CODA_HOME` (compat), otherwise `~/.lar`.
pub fn home() -> PathBuf {
    for key in ["LAR_HOME", "CODA_HOME"] {
        if let Ok(forced) = std::env::var(key) {
            let trimmed = forced.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
    }
    user_home().join(".lar")
}

pub fn agent_dir() -> PathBuf {
    home().join("agent")
}

pub fn models_dir() -> PathBuf {
    home().join("models")
}

pub fn logs_dir() -> PathBuf {
    home().join("logs")
}

/// Copy Ghost Note / Lar app-data into `~/.lar` without deleting the source or
/// overwriting files that Lar already wrote.
pub fn migrate_from_legacy() -> io::Result<MigrateReport> {
    let dest = home();
    fs::create_dir_all(&dest)?;
    let marker = dest.join(MARKER);
    if marker.is_file() {
        return Ok(MigrateReport {
            copied: false,
            from: None,
            reason: "already migrated",
        });
    }

    let Some(from) = legacy_data_dir() else {
        write_marker(&marker, None)?;
        return Ok(MigrateReport {
            copied: false,
            from: None,
            reason: "no prior data directory",
        });
    };
    if !from.is_dir() {
        write_marker(&marker, Some(&from))?;
        return Ok(MigrateReport {
            copied: false,
            from: Some(from),
            reason: "legacy directory missing",
        });
    }

    let copied = copy_tree(&from, &dest)?;
    // Prefer the new audit filename when an older Lar log was copied.
    let old_audit = dest.join("lar_audit.log");
    let new_audit = dest.join("lar_audit.log");
    if old_audit.is_file() && !new_audit.is_file() {
        let _ = fs::rename(&old_audit, &new_audit);
    }
    write_marker(&marker, Some(&from))?;
    Ok(MigrateReport {
        copied,
        from: Some(from),
        reason: "copied",
    })
}

#[derive(Debug, Clone)]
pub struct MigrateReport {
    pub copied: bool,
    pub from: Option<PathBuf>,
    pub reason: &'static str,
}

fn write_marker(path: &Path, from: Option<&Path>) -> io::Result<()> {
    let body = format!(
        "from={}\n",
        from.map(|p| p.display().to_string()).unwrap_or_default()
    );
    fs::write(path, body)
}

fn legacy_data_dir() -> Option<PathBuf> {
    for key in ["LAR_LEGACY_DATA", "CODA_LEGACY_DATA"] {
        if let Ok(forced) = std::env::var(key) {
            if !forced.trim().is_empty() {
                return Some(PathBuf::from(forced));
            }
        }
    }
    let home = user_home();
    let prior = home.join(PRIOR_CODA_DIR);
    if prior.is_dir() {
        return Some(prior);
    }
    if cfg!(target_os = "macos") {
        Some(
            home.join("Library")
                .join("Application Support")
                .join(GHOSTNOTE_BUNDLE),
        )
    } else if cfg!(target_os = "windows") {
        Some(
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("AppData").join("Roaming"))
                .join(GHOSTNOTE_BUNDLE),
        )
    } else {
        Some(
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local").join("share"))
                .join(GHOSTNOTE_BUNDLE),
        )
    }
}

fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Copy `from` into `to`. Existing dest files are left untouched.
fn copy_tree(from: &Path, to: &Path) -> io::Result<bool> {
    let mut copied_any = false;
    let mut stack = vec![from.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let src = entry.path();
            let rel = src.strip_prefix(from).unwrap_or(&src);
            let dest = to.join(rel);
            if src.is_dir() {
                fs::create_dir_all(&dest)?;
                stack.push(src);
            } else if !dest.is_file() {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&src, &dest)?;
                copied_any = true;
            }
        }
    }
    Ok(copied_any)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::new(());

    fn unique_temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lar-mig-{}-{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp");
        dir
    }

    #[test]
    fn copies_legacy_without_deleting_source_or_overwriting() {
        let _guard = LOCK.lock().expect("lock");
        let root = unique_temp("copy");
        let legacy = root.join("legacy");
        let dest = root.join("lar");
        fs::create_dir_all(legacy.join("agent")).unwrap();
        fs::write(legacy.join("agent/audit.jsonl"), b"old-audit\n").unwrap();
        fs::write(legacy.join("agent/state.json"), b"{\"tasks\":[]}").unwrap();
        fs::write(legacy.join("lar_audit.log"), b"old-log\n").unwrap();
        fs::create_dir_all(dest.join("agent")).unwrap();
        fs::write(dest.join("agent/state.json"), b"{\"keep\":true}").unwrap();

        std::env::set_var("LAR_HOME", dest.to_string_lossy().as_ref());
        std::env::set_var("LAR_LEGACY_DATA", legacy.to_string_lossy().as_ref());
        let report = migrate_from_legacy().expect("migrate");
        assert!(report.copied);
        assert_eq!(
            fs::read_to_string(dest.join("agent/state.json")).unwrap(),
            "{\"keep\":true}"
        );
        assert_eq!(
            fs::read_to_string(dest.join("agent/audit.jsonl")).unwrap(),
            "old-audit\n"
        );
        assert_eq!(
            fs::read_to_string(dest.join("lar_audit.log")).unwrap(),
            "old-log\n"
        );
        assert!(legacy.join("agent/audit.jsonl").is_file());
        assert!(dest.join(MARKER).is_file());

        let again = migrate_from_legacy().expect("second");
        assert!(!again.copied);
        assert_eq!(again.reason, "already migrated");

        std::env::remove_var("LAR_HOME");
        std::env::remove_var("LAR_LEGACY_DATA");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skip_when_legacy_missing() {
        let _guard = LOCK.lock().expect("lock");
        let dest = unique_temp("nolegacy");
        std::env::set_var("LAR_HOME", dest.to_string_lossy().as_ref());
        std::env::set_var(
            "LAR_LEGACY_DATA",
            dest.join("does-not-exist").to_string_lossy().as_ref(),
        );
        let report = migrate_from_legacy().expect("migrate");
        assert!(!report.copied);
        assert!(dest.join(MARKER).is_file());
        std::env::remove_var("LAR_HOME");
        std::env::remove_var("LAR_LEGACY_DATA");
        let _ = fs::remove_dir_all(dest);
    }
}
