//! Authoring kit for **appcast transporter plugins**.
//!
//! appcast loads transporter backends from shared libraries at runtime
//! (`~/.config/appcast/transporters/libappcast_tpt_*.so` and friends). Rust
//! has no stable ABI, so plugins never exchange Rust types with the host:
//! this crate pins a narrow **C ABI** where every payload travels as JSON
//! strings, then hides it behind a plain Rust trait plus one export macro.
//! Plugin authors implement [`SimpleTransporter`] and call
//! [`export_appcast_transporter!`] — no `unsafe` anywhere on their side.
//!
//! # Wire contract (C ABI v1)
//!
//! Every plugin exports exactly these symbols; the host checks the version
//! symbol *first* and refuses anything it does not understand:
//!
//! ```text
//! uint32_t    appcast_tpt_abi_version(void)
//! const char* appcast_tpt_name(void)                       // registry name
//! int32_t     appcast_tpt_run(const char* config_json)     // 0 = success
//! const char* appcast_tpt_list_apps(const char* target,
//!                                   const char* params_json) // JSON array or NULL
//! void        appcast_tpt_free_string(char*)               // frees plugin-owned strings
//! ```
//!
//! Memory rules: every string a plugin returns is allocated by the plugin
//! and must be released through the plugin's own `appcast_tpt_free_string`
//! (never free across module boundaries). The host copies what it needs
//! before freeing. Error reporting goes to stderr from inside the plugin;
//! non-zero run status / NULL list result signal failure to the host.
//!
//! Because calls cross the boundary as blocking C functions, plugin methods
//! are synchronous; the host wraps them in `spawn_blocking`.
//!
//! # Example plugin
//!
//! ```no_run
//! use std::collections::HashMap;
//! use appcast_plugin::{
//!     export_appcast_transporter, ConfigSnapshot, ListedApp, SimpleTransporter,
//! };
//!
//! struct Echo;
//!
//! impl SimpleTransporter for Echo {
//!     fn name(&self) -> &'static str { "echo" }
//!
//!     fn run(&self, _config: ConfigSnapshot) -> Result<(), String> {
//!         eprintln!("echo would cast now");
//!         Ok(())
//!     }
//!
//!     fn list_apps(
//!         &self,
//!         _target: &str,
//!         _params: &HashMap<String, String>,
//!     ) -> Result<Vec<ListedApp>, String> {
//!         Ok(vec![ListedApp::id_only("demo")])
//!     }
//! }
//!
//! export_appcast_transporter!(Echo);
//! ```
//!
//! Build with `crate-type = ["cdylib"]`, name the artifact
//! `libappcast_tpt_<name>.{so,dylib,dll}` and drop it into the plugin
//! directory — done.

use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;

// Re-exported so the export macro can reference it as `$crate::serde_json`
// without plugin crates needing their own serde_json dependency.
#[doc(hidden)]
pub use serde_json;

/// Wire protocol version. Bump on any breaking change to the symbol set or
/// payload schemas; hosts refuse mismatching plugins at load time.
pub const APPCAST_TPT_ABI_VERSION: u32 = 1;

/// The fully merged configuration, mirrored from the host's
/// `ResolvedConfig`. Field-for-field identical JSON shape.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ConfigSnapshot {
    /// Registry name of the selected transporter (this plugin's own name).
    pub transporter: String,
    /// Address slot ("where"): URL, serial, host, ... as defined by you.
    pub target: Option<String>,
    /// Content slot ("what to open there"); optional per transporter.
    pub app: Option<String>,
    /// Free-form extension params (`--param KEY=VALUE` / profile `params`).
    pub params: HashMap<String, String>,
    /// Passthrough args appended verbatim by convention (`-- ...`).
    pub raw_args: Vec<String>,
}

impl ConfigSnapshot {
    /// Look up an extension param by key.
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(String::as_str)
    }
}

/// One enumerable item on a target — mirror of the host's `AppEntry`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ListedApp {
    /// Canonical identifier to feed back into the `app` slot of `run`.
    pub id: String,
    /// Human-readable display name, when obtainable.
    pub name: Option<String>,
    /// Backend-specific extras; consumers treat unknown keys as opaque.
    pub meta: HashMap<String, String>,
}

impl ListedApp {
    /// Bare-bones entry with no display name and no extras.
    pub fn id_only(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: None,
            meta: HashMap::new(),
        }
    }
}

/// The blocking, owned-data counterpart of the host's async `Transporter`
/// trait. Implement this, then register your type via
/// [`export_appcast_transporter!`].
///
/// All methods run on a blocking host thread and may take as long as they
/// need (`run` spans the whole casting session). They must be safe to call
/// from multiple threads concurrently.
pub trait SimpleTransporter: Send + Sync {
    /// Registry name of this transporter (e.g. `"web-webview"`).
    fn name(&self) -> &'static str;

    /// Run one full casting session. Return `Err` only for genuine
    /// failures; normal session end (window closed, user interrupt) is
    /// `Ok(())`.
    fn run(&self, config: ConfigSnapshot) -> Result<(), String>;

    /// List application entries available on `target`. Returning an empty
    /// vector is legitimate when a target has nothing enumerable.
    fn list_apps(
        &self,
        target: &str,
        params: &HashMap<String, String>,
    ) -> Result<Vec<ListedApp>, String>;
}

/// Export the C ABI v1 symbols for one [`SimpleTransporter`] implementation.
///
/// `$factory` is an expression producing your transporter; it is evaluated
/// lazily, once, at the first symbol call. All FFI glue — including the
/// required `free_string` — is generated here.
#[macro_export]
macro_rules! export_appcast_transporter {
    ($factory:expr) => {
        const _: () = {
        static INSTANCE: ::std::sync::OnceLock<
            ::std::boxed::Box<dyn $crate::SimpleTransporter>,
        > = ::std::sync::OnceLock::new();
        static NAME: ::std::sync::OnceLock<::std::ffi::CString> =
            ::std::sync::OnceLock::new();

        fn instance() -> &'static dyn $crate::SimpleTransporter {
            INSTANCE
                .get_or_init(|| ::std::boxed::Box::new($factory))
                .as_ref()
        }

        /// Hosts check this first and refuse mismatching versions.
        #[no_mangle]
        extern "C" fn appcast_tpt_abi_version() -> u32 {
            $crate::APPCAST_TPT_ABI_VERSION
        }

        /// Called once per load; the string lives for the whole process.
        #[no_mangle]
        extern "C" fn appcast_tpt_name() -> *const ::std::os::raw::c_char {
            NAME.get_or_init(|| {
                ::std::ffi::CString::new(instance().name())
                    .expect("transporter name contains no NUL")
            })
            .as_ptr()
        }

        #[no_mangle]
        extern "C" fn appcast_tpt_run(config_json: *const ::std::os::raw::c_char) -> i32 {
            let Some(config) = (unsafe { $crate::__read_json::<$crate::ConfigSnapshot>(config_json) })
            else {
                return -1;
            };
            match instance().run(config) {
                Ok(()) => 0,
                Err(message) => {
                    eprintln!("appcast plugin error: {message}");
                    -1
                }
            }
        }

        #[no_mangle]
        extern "C" fn appcast_tpt_list_apps(
            target: *const ::std::os::raw::c_char,
            params_json: *const ::std::os::raw::c_char,
        ) -> *mut ::std::os::raw::c_char {
            let outcome: Result<*mut ::std::os::raw::c_char, String> = (|| {
                let target = unsafe { $crate::__read_str(target) }.ok_or_else(|| "null target".to_string())?;
                let params = unsafe {
                        $crate::__read_json::<HashMap<String, String>>(params_json)
                    }
                        .ok_or_else(|| "invalid params JSON".to_string())?;
                let apps = instance().list_apps(&target, &params)?;
                let json =
                    $crate::serde_json::to_string(&apps).map_err(|e| e.to_string())?;
                Ok(::std::ffi::CString::new(json)
                    .map_err(|e| e.to_string())?
                    .into_raw())
            })();
            match outcome {
                Ok(ptr) => ptr,
                Err(err) => {
                    eprintln!("appcast plugin error: list_apps: {err}");
                    ::std::ptr::null_mut()
                }
            }
        }

        /// Strings returned by this plugin are freed here, keeping every
        /// allocation inside its own module boundary.
        #[no_mangle]
        extern "C" fn appcast_tpt_free_string(string: *mut ::std::os::raw::c_char) {
            if !string.is_null() {
                drop(unsafe { ::std::ffi::CString::from_raw(string) });
            }
        }
        };
    };
}

/// Read a host-provided C string. The host contract guarantees the pointer
/// is null or valid NUL-terminated data for the duration of the call; we
/// copy out immediately so nothing outlives the borrow.
///
/// # Safety
/// The host guarantees `ptr` is null or valid NUL-terminated data for the
/// duration of the call.
#[doc(hidden)]
pub unsafe fn __read_str(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        None
    } else {
        Some(CStr::from_ptr(ptr).to_string_lossy().into_owned())
    }
}

/// Deserialize a host-provided JSON payload.
/// # Safety
/// See [`__read_str`].
#[doc(hidden)]
pub unsafe fn __read_json<T: serde::de::DeserializeOwned>(ptr: *const c_char) -> Option<T> {
    let raw = __read_str(ptr)?;
    serde_json::from_str(&raw).ok()
}
