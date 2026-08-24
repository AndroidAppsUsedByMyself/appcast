//! Profile model + XDG storage (plain YAML files, stateless-tolerant IO).
//!
//! `appcast run` never touches the filesystem unless `--profile` is given;
//! listing a missing directory is an empty list, not an error.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use etcetera::base_strategy::{choose_base_strategy, BaseStrategy};
use serde::{Deserialize, Serialize};

use crate::core::error::AppError;

/// A saved parameter bundle. `transporter` is required; the addressing slots
/// and everything else default, so hand-written YAML stays minimal.
///
/// Backend-specific knobs (resolution, fps, bit_rate, paths, ...) live in
/// `params` — the selected backend interprets them and owns the defaults.
/// Legacy profiles that still carry top-level `resolution:`/`fps:`/... keys
/// keep loading fine; those keys are ignored.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Profile {
    /// `"adb"` | `"ssh-x11"` | `"waypipe"` | ...
    pub transporter: String,
    /// Address slot ("where"); optional — required by most transporters.
    #[serde(default)]
    pub target: Option<String>,
    /// Content slot ("what to open there"); optional per transporter.
    #[serde(default)]
    pub app: Option<String>,
    /// Free-form extension params, overridable key-by-key via `--param`.
    #[serde(default)]
    pub params: HashMap<String, String>,
    /// Passthrough args appended verbatim to the backend command
    /// (`-- ...`); CLI passthrough replaces this when non-empty.
    #[serde(default)]
    pub raw_args: Vec<String>,
}

/// Skeleton written by `profile edit` when the profile does not exist yet.
pub const PROFILE_TEMPLATE: &str = r#"# AppCast profile
transporter: adb-scrcpy
# Which slots are required depends on the transporter (adb needs both):
target: ""
app: ""
# Backend params — interpreted by the selected transporter; unset keys fall
# back to backend defaults. adb/scrcpy understands:
params:
  # resolution: 1920x1080   # virtual display size, <W>x<H>
  # fps: 60                 # frame rate cap
  # bit_rate: 8             # video bit rate in Mbps
  # adb_path / scrcpy_path: custom binaries
raw_args: []                  # verbatim scrcpy flags, e.g. ["-x", "--no-audio"]
"#;

/// `$XDG_CONFIG_HOME/appcast` root directory.
fn config_root() -> Result<PathBuf, AppError> {
    let strategy = choose_base_strategy().map_err(|e| {
        AppError::Io(std::io::Error::other(format!("cannot locate config home: {e}")))
    })?;
    Ok(strategy.config_dir().join("appcast"))
}

/// `$XDG_CONFIG_HOME/appcast/profiles`.
pub fn profiles_dir() -> Result<PathBuf, AppError> {
    Ok(config_root()?.join("profiles"))
}

/// File path for a profile; `/` and `\` in names are neutralized so a name
/// can never escape the profiles directory.
fn profile_path(dir: &Path, name: &str) -> PathBuf {
    let safe_name = name.replace(['/', '\\'], "_");
    dir.join(format!("{safe_name}.yaml"))
}

/// Load a profile from an explicit directory (test-friendly variant).
pub fn load_profile_in(dir: &Path, name: &str) -> Result<Profile, AppError> {
    let path = profile_path(dir, name);
    if !path.exists() {
        return Err(AppError::ProfileNotFound(name.to_string()));
    }
    let raw = fs::read_to_string(&path)?;
    Ok(serde_yaml::from_str(&raw)?)
}

/// Load `<NAME>.yaml` from the XDG profiles directory.
pub fn load_profile(name: &str) -> Result<Profile, AppError> {
    load_profile_in(&profiles_dir()?, name)
}

/// Save (overwriting) a profile into `dir`; creates the directory if needed.
pub fn save_profile_in(dir: &Path, name: &str, profile: &Profile) -> Result<PathBuf, AppError> {
    fs::create_dir_all(dir)?;
    let path = profile_path(dir, name);
    fs::write(&path, serde_yaml::to_string(profile)?)?;
    Ok(path)
}

/// Save a profile under the XDG profiles directory.
pub fn save_profile(name: &str, profile: &Profile) -> Result<PathBuf, AppError> {
    save_profile_in(&profiles_dir()?, name, profile)
}

/// Delete a profile from an explicit directory (test-friendly variant).
pub fn delete_profile_in(dir: &Path, name: &str) -> Result<(), AppError> {
    let path = profile_path(dir, name);
    if !path.exists() {
        return Err(AppError::ProfileNotFound(name.to_string()));
    }
    fs::remove_file(&path)?;
    Ok(())
}

/// Delete `<NAME>.yaml` from the XDG profiles directory.
pub fn delete_profile(name: &str) -> Result<(), AppError> {
    delete_profile_in(&profiles_dir()?, name)
}

/// List profile names in `dir`; a missing directory means "no profiles".
pub fn list_profiles_in(dir: &Path) -> Result<Vec<String>, AppError> {
    let mut names = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(names),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let is_yaml = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext == "yaml" || ext == "yml");
        if is_yaml {
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

/// List all saved profile names, sorted.
pub fn list_profiles() -> Result<Vec<String>, AppError> {
    list_profiles_in(&profiles_dir()?)
}

/// Editor launch candidates, highest priority first.
///
/// `$VISUAL`/`$EDITOR` win (whitespace-split so values like `code -w`
/// work), followed by a per-platform guaranteed-ish default: `notepad` on
/// Windows, `nano`/`vi` on Unix.
fn editor_candidates() -> Vec<Vec<String>> {
    let mut candidates = Vec::new();
    for key in ["VISUAL", "EDITOR"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            let mut parts = value.split_whitespace().map(str::to_owned);
            let program = parts.next().expect("non-empty value has a first token");
            candidates.push(std::iter::once(program).chain(parts).collect::<Vec<_>>());
        }
    }
    if cfg!(windows) {
        candidates.push(vec!["notepad".to_string()]);
    } else {
        candidates.push(vec!["nano".to_string()]);
        candidates.push(vec!["vi".to_string()]);
    }
    candidates
}

/// Ensure the profile exists (write the template if missing), then open it
/// in the user's editor. Blocks until the editor exits.
pub async fn edit_profile(name: &str) -> Result<(), AppError> {
    let dir = profiles_dir()?;
    let path = profile_path(&dir, name);
    if !path.exists() {
        let template: Profile = serde_yaml::from_str(PROFILE_TEMPLATE)?;
        save_profile_in(&dir, name, &template)?;
    }

    // Try every candidate; "program not found" moves down the chain, while
    // a launched-but-failing editor surfaces its own status.
    let candidates = editor_candidates();
    let mut not_found = Vec::new();
    for candidate in &candidates {
        let (program, args) = candidate.split_first().expect("candidate has a program");
        match tokio::process::Command::new(program)
            .args(args)
            .arg(&path)
            .status()
            .await
        {
            Ok(status) => {
                if !status.success() {
                    return Err(AppError::BackendError(format!(
                        "editor `{program}` exited with {status}"
                    )));
                }
                return Ok(());
            }
            Err(e) => not_found.push(format!("{program}: {e}")),
        }
    }

    Err(AppError::BackendError(format!(
        "no usable editor found (tried: {}); set $VISUAL or $EDITOR",
        not_found.join("; ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Profile {
        Profile {
            transporter: "adb".into(),
            target: Some("emulator-5554".into()),
            app: Some("com.tencent.mm".into()),
            params: HashMap::from([("adb_path".to_string(), "/custom/adb".to_string())]),
            raw_args: vec![],
        }
    }

    #[test]
    fn roundtrip_save_load_delete() {
        let dir = tempfile::tempdir().unwrap();
        let p = sample();

        save_profile_in(dir.path(), "wechat", &p).unwrap();
        assert_eq!(list_profiles_in(dir.path()).unwrap(), vec!["wechat"]);

        let loaded = load_profile_in(dir.path(), "wechat").unwrap();
        assert_eq!(loaded.transporter, "adb");
        assert_eq!(loaded.params.get("adb_path").map(String::as_str), Some("/custom/adb"));

        delete_profile_in(dir.path(), "wechat").unwrap();
        assert!(load_profile_in(dir.path(), "wechat").is_err());
    }

    #[test]
    fn missing_profile_is_not_found_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_profile_in(dir.path(), "nope").unwrap_err();
        assert!(matches!(err, AppError::ProfileNotFound(name) if name == "nope"));
    }

    #[test]
    fn missing_directory_lists_empty() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("does-not-exist");
        assert_eq!(list_profiles_in(&empty).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn slashes_in_names_cannot_escape_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = save_profile_in(dir.path(), "../evil", &sample()).unwrap();
        assert!(path.starts_with(dir.path()));
        // `../evil` becomes the literal file name `.._evil.yaml` inside `dir`.
        assert_eq!(path.file_name().unwrap(), ".._evil.yaml");
    }

    #[test]
    fn template_parses_into_valid_profile() {
        let parsed: Profile = serde_yaml::from_str(PROFILE_TEMPLATE).unwrap();
        assert_eq!(parsed.transporter, "adb-scrcpy");
        assert!(parsed.params.is_empty());
        assert!(parsed.raw_args.is_empty());
    }

    #[test]
    fn editor_chain_always_has_a_platform_fallback() {
        let candidates = editor_candidates();
        assert!(!candidates.is_empty());
        let last = candidates.last().unwrap().join(" ");
        if cfg!(windows) {
            assert_eq!(last, "notepad");
        } else {
            assert!(last == "vi" || last == "nano");
        }
    }

    #[test]
    fn legacy_profile_fields_are_tolerated_and_ignored() {
        // Pre-slim profiles carried typed top-level knobs; they must still
        // load (serde ignores unknown fields) with the values dropped — the
        // backend defaults take over instead.
        let legacy = r#"
transporter: adb
target: serial
app: com.a.b
activity: .Main
resolution: 1280x720
fps: 30
bit_rate: 4
params:
  keep: yes
"#;
        let parsed: Profile = serde_yaml::from_str(legacy).unwrap();
        assert_eq!(parsed.target.as_deref(), Some("serial"));
        assert!(parsed.params.contains_key("keep"));
    }
}
