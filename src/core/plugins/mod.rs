//! Runtime discovery and loading of out-of-tree transporter plugins.
//!
//! Rust has no stable ABI, so plugins speak the narrow C ABI pinned by the
//! `appcast-plugin` SDK crate (see `sdk/appcast-plugin`): version
//! handshake first, JSON strings as payloads, plugin-owned allocation
//! freed through the plugin's own `free_string`. This module maps that
//! contract back onto the native async [`Transporter`] trait via
//! [`DynamicTransporter`].
//!
//! Discovery: `$APPCAST_TRANSPORTER_DIR` (PATH-style, split like `$PATH`
//! so NixOS profiles can point at store paths) or the default XDG config
//! location `~/.config/appcast/transporters/`. Only files named after the
//! convention `libappcast_tpt_*.{so,dylib,dll}` are attempted; a broken
//! plugin warns and is skipped rather than blocking the others.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use etcetera::base_strategy::{choose_base_strategy, BaseStrategy};
use libloading::Library;
use tracing::{debug, info, warn};

use crate::core::error::AppError;
use crate::core::registry::TransporterRegistry;
use crate::core::transporter::{AppEntry, BoxFut, ResolvedConfig, Transporter};

/// The only ABI this build understands; keep in lockstep with
/// `appcast_plugin::APPCAST_TPT_ABI_VERSION`.
const SUPPORTED_ABI_VERSION: u32 = 1;

/// File-name stem of loadable plugins; artifacts are
/// `appcast_tpt_<name>.<dll>` (Windows) or `libappcast_tpt_<name>.<dll>`
/// (Unix cdylib convention). The platform suffix comes from
/// `std::env::consts::DLL_SUFFIX`.
const PLUGIN_FILE_STEM: &str = "appcast_tpt_";

// C ABI v1 signatures — must mirror sdk/appcast-plugin exactly.
type AbiVersionFn = unsafe extern "C" fn() -> u32;
type NameFn = unsafe extern "C" fn() -> *const c_char;
type RunFn = unsafe extern "C" fn(*const c_char) -> i32;
type ListAppsFn = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_char;
type FreeStringFn = unsafe extern "C" fn(*mut c_char);

/// Directories to scan, in order. `$APPCAST_TRANSPORTER_DIR` (PATH-style,
/// split like `$PATH`) replaces — not extends — the default, so a hermetic
/// setup can opt out of user dirs entirely.
pub(crate) fn search_dirs() -> Vec<PathBuf> {
    dirs_from(std::env::var_os("APPCAST_TRANSPORTER_DIR"))
}

/// Pure core of [`search_dirs`], injectable so tests never touch process
/// environment state.
fn dirs_from(env_value: Option<std::ffi::OsString>) -> Vec<PathBuf> {
    if let Some(raw) = env_value {
        std::env::split_paths(&raw).collect()
    } else {
        // Mirrors config::profile's namespace: <config>/appcast/transporters.
        match choose_base_strategy().map(|s| s.config_dir().join("appcast").join("transporters")) {
            Ok(dir) => vec![dir],
            Err(_) => Vec::new(),
        }
    }
}

/// Naming-convention filter: optional `lib` prefix (Linux/macOS artifacts)
/// plus the fixed stem and the platform's shared-library suffix.
fn is_plugin_file(file_name: &str) -> bool {
    let Some(stem) = file_name.strip_suffix(std::env::consts::DLL_SUFFIX) else {
        return false;
    };
    let lib_prefixed = format!("lib{PLUGIN_FILE_STEM}");
    stem.starts_with(PLUGIN_FILE_STEM) || stem.starts_with(&lib_prefixed)
}

/// All candidate plugin files across the configured dirs, sorted for
/// deterministic load order (later files win name collisions).
fn discover_files() -> Vec<PathBuf> {
    discover_in(&search_dirs())
}

fn discover_in(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            debug!(dir = %dir.display(), "plugin dir absent or unreadable");
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(is_plugin_file)
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

/// A transporter whose implementation lives in a shared library.
///
/// Owns the [`Library`] to keep it mapped for the instance's whole life.
/// `Send + Sync`: symbol lookup through a loaded library is thread-safe,
/// and the SDK contract requires plugin functions to be callable from any
/// thread (the host invokes them on blocking worker threads).
struct DynamicTransporter {
    /// Registry name reported by the plugin itself.
    name: String,
    _library: Library,
    abi_version: u32,
    run_fn: RunFn,
    list_apps_fn: ListAppsFn,
    free_string_fn: FreeStringFn,
}

unsafe impl Send for DynamicTransporter {}
unsafe impl Sync for DynamicTransporter {}

impl DynamicTransporter {
    /// Load one plugin file: handshake the ABI, read the registry name,
    /// resolve every required symbol. Any gap refuses just this file.
    fn load(path: &Path) -> Result<Self, String> {
        let library = unsafe { Library::new(path) }
            .map_err(|e| format!("cannot load: {e}"))?;

        // Version check strictly before touching anything else.
        let abi_version = {
            let version_fn: libloading::Symbol<AbiVersionFn> = unsafe {
                library.get(b"appcast_tpt_abi_version\0")
            }
            .map_err(|_| "missing appcast_tpt_abi_version".to_string())?;
            unsafe { version_fn() }
        };
        if abi_version != SUPPORTED_ABI_VERSION {
            return Err(format!(
                "ABI version {abi_version} not supported (this appcast speaks {SUPPORTED_ABI_VERSION})"
            ));
        }

        let name = {
            let name_fn: libloading::Symbol<NameFn> =
                unsafe { library.get(b"appcast_tpt_name\0") }
                    .map_err(|_| "missing appcast_tpt_name".to_string())?;
            let name_ptr = unsafe { name_fn() };
            (!name_ptr.is_null())
                .then(|| unsafe { CStr::from_ptr(name_ptr) }.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
                .ok_or("reported empty or null name")?
        };

        // Copy every fn pointer out, ending the Symbol borrows, so the
        // Library itself can move into the returned instance afterwards.
        let (run_fn, list_apps_fn, free_string_fn) = {
            let run_fn: libloading::Symbol<RunFn> =
                unsafe { library.get(b"appcast_tpt_run\0") }
                    .map_err(|_| "missing appcast_tpt_run".to_string())?;
            let list_apps_fn: libloading::Symbol<ListAppsFn> =
                unsafe { library.get(b"appcast_tpt_list_apps\0") }
                    .map_err(|_| "missing appcast_tpt_list_apps".to_string())?;
            // Mandatory: without it we could never release returned strings.
            let free_string_fn: libloading::Symbol<FreeStringFn> =
                unsafe { library.get(b"appcast_tpt_free_string\0") }
                    .map_err(|_| "missing appcast_tpt_free_string".to_string())?;
            (*run_fn, *list_apps_fn, *free_string_fn)
        };

        Ok(Self {
            name,
            _library: library,
            abi_version,
            run_fn,
            list_apps_fn,
            free_string_fn,
        })
    }

    /// Serialize `value`, hand it across the boundary, reclaim ownership.
    fn to_cstring(value: impl serde::Serialize, what: &str) -> Result<CString, AppError> {
        let json = serde_json::to_string(&value).map_err(|e| {
            AppError::BackendError(format!("plugin: serialize {what}: {e}"))
        })?;
        CString::new(json)
            .map_err(|_| AppError::BackendError(format!("plugin: NUL in {what}")))
    }
}

impl Transporter for DynamicTransporter {
    fn name(&self) -> &'static str {
        // The trait promises 'static; plugin names live for the process
        // anyway, so leak once per loaded plugin (bounded by plugin count).
        Box::leak(self.name.clone().into_boxed_str())
    }

    fn run<'a>(&'a self, config: &'a ResolvedConfig) -> BoxFut<'a, Result<(), AppError>> {
        Box::pin(async move {
            let payload = Self::to_cstring(config, "config")?;
            let run_fn = self.run_fn;
            let plugin = self.name.clone();
            // Blocking C call → blocking pool, keeping the async runtime
            // responsive for the whole session.
            let code = tokio::task::spawn_blocking(move || unsafe { run_fn(payload.as_ptr()) })
                .await
                .map_err(|e| {
                    AppError::BackendError(format!("plugin `{plugin}`: worker failed: {e}"))
                })?;
            if code == 0 {
                Ok(())
            } else {
                Err(AppError::BackendError(format!(
                    "plugin `{plugin}` exited with status {code} (details on stderr)"
                )))
            }
        })
    }

    fn list_apps<'a>(
        &'a self,
        target: &'a str,
        params: &'a std::collections::HashMap<String, String>,
    ) -> BoxFut<'a, Result<Vec<AppEntry>, AppError>> {
        Box::pin(async move {
            let target = Self::to_cstring(target, "target")?;
            let params = Self::to_cstring(params, "params")?;
            let list_apps_fn = self.list_apps_fn;
            let free_string_fn = self.free_string_fn;
            // One clone lives inside the blocking closure, one reports the
            // join error outside it.
            let plugin = self.name.clone();
            let plugin_label = self.name.clone();

            tokio::task::spawn_blocking(move || unsafe {
                let raw = list_apps_fn(target.as_ptr(), params.as_ptr());
                if raw.is_null() {
                    return Err(AppError::BackendError(format!(
                        "plugin `{plugin}`: list_apps failed (details on stderr)"
                    )));
                }
                // Copy before freeing — the bytes die with free_string.
                let bytes = CStr::from_ptr(raw).to_bytes();
                let parsed: Result<Vec<AppEntry>, serde_json::Error> =
                    serde_json::from_slice(bytes);
                free_string_fn(raw);
                parsed.map_err(|e| {
                    AppError::BackendError(format!("plugin `{plugin}`: bad listing JSON: {e}"))
                })
            })
            .await
            .map_err(|e| {
                AppError::BackendError(format!("plugin `{plugin_label}`: worker failed: {e}"))
            })?
        })
    }
}

/// Load every discovered plugin into `registry`. Failures are per-file:
/// each problem warns with its path and loading continues.
pub fn load_into(registry: &mut TransporterRegistry) {
    for path in discover_files() {
        match DynamicTransporter::load(&path) {
            Ok(transporter) => {
                info!(
                    name = %transporter.name,
                    abi = transporter.abi_version,
                    source = %path.display(),
                    "loaded transporter plugin"
                );
                let name = transporter.name.clone();
                registry.register_plugin(&name, Arc::new(transporter), path);
            }
            Err(reason) => {
                warn!(
                    source = %path.display(),
                    %reason,
                    "skipping unusable transporter plugin"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_matches_both_artifact_spellings() {
        let dll = std::env::consts::DLL_SUFFIX; // may or may not carry the dot
        let bare = dll.trim_start_matches('.');
        let other_dll = if bare == "so" { "dylib" } else { "so" };
        // Unix cdylib artifacts carry the `lib` prefix...
        assert!(is_plugin_file(&format!("libappcast_tpt_echo.{dll}")));
        // ...Windows ones do not; both must load on their own platform.
        assert!(is_plugin_file(&format!("appcast_tpt_echo.{dll}")));
        // Foreign-platform artifacts are inert here — one binary per OS
        // loads its own kind only.
        assert!(!is_plugin_file(&format!(
            "libappcast_tpt_echo.{bare}{other_dll}"
        )));
        // Everything else stays out of dlopen range.
        assert!(!is_plugin_file(&format!("random.{dll}")));
        assert!(!is_plugin_file("libappcast.so"));
        assert!(!is_plugin_file("appcast_tpt_notes.txt"));
        assert!(!is_plugin_file(&format!("libappcast_tpt_echo.{dll}.bak")));
    }

    #[test]
    fn env_override_replaces_default_with_split_paths() {
        let dirs = dirs_from(Some(std::ffi::OsString::from(
            "/a/plugins:/b/more",
        )));
        assert_eq!(
            dirs,
            vec![PathBuf::from("/a/plugins"), PathBuf::from("/b/more")]
        );
        // Unset falls back to exactly one default dir.
        assert_eq!(dirs_from(None).len(), 1);
    }

    #[test]
    fn discovery_selects_and_sorts_across_dirs() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("z-dir");
        let second = root.path().join("a-dir");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();

        for (dir, name) in [
            (&first, "noise.so"),
            (&first, "libappcast_tpt_beta.so"),
            (&second, "libappcast_tpt_alpha.so"),
            (&second, "sub"),
        ] {
            let path = dir.join(name);
            if name == "sub" {
                std::fs::create_dir_all(path).unwrap();
            } else {
                std::fs::write(path, b"").unwrap();
            }
        }

        let found = discover_in(&[first.clone(), second.clone()]);
        assert_eq!(
            found,
            vec![
                second.join("libappcast_tpt_alpha.so"),
                first.join("libappcast_tpt_beta.so"),
            ]
        );
    }
}
