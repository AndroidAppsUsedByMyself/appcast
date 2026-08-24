//! Core abstractions shared by every backend: the resolved config and the
//! `Transporter` trait.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use crate::core::error::AppError;

/// Hand-boxed future alias. Boxing keeps the trait object-safe
/// (`Box<dyn Transporter>` for the registry) without pulling in
/// `async-trait` as an extra dependency.
pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The final, fully merged configuration — the only input shape the core
/// layer accepts. Frontends (CLI today, TUI/Web later) are responsible for
/// producing it via the priority stack.
///
/// Deliberately thin. The addressing slots are *optional* here because their
/// arity is backend-defined: adb needs `<TARGET> <APP>`, a future web
/// transporter needs only a URL, a local-window capturer has no target at
/// all. Each backend validates the slots it requires and owns its usage
/// message; everything backend-specific beyond the slots travels inside
/// `params` (with backend-owned defaults).
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    /// Protocol name, e.g. `"adb"`.
    pub transporter: String,
    /// Address slot ("where"): device serial, host, URL, ... as defined by
    /// the backend.
    pub target: Option<String>,
    /// Content slot ("what to open there"): package name, executable path,
    /// ... as defined by the backend.
    pub app: Option<String>,
    /// Free-form extension params; keys are backend-defined
    /// (`adb_path`, `resolution`, `fps`, ...).
    pub params: HashMap<String, String>,
    /// Raw args to append verbatim to the backend command (`-- ...`).
    pub raw_args: Vec<String>,
}

impl ResolvedConfig {
    /// Look up an extension param by key.
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(String::as_str)
    }
}

/// One enumerable item on a target — the typed result of `list`.
///
/// This is a core-level data API, not a printing helper: every frontend
/// (CLI today, TUI/WebUI later) consumes these values and renders them its
/// own way. `Serialize` lets the CLI expose `--json` today and lets a WebUI
/// return them as JSON verbatim tomorrow.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AppEntry {
    /// Canonical identifier to feed back into the `app` slot of `run`
    /// (Android package name, executable path, ...).
    pub id: String,
    /// Human-readable display name, when the backend can obtain one
    /// (`None` keeps script-friendly bare-id listings honest).
    pub name: Option<String>,
    /// Reserved for backend-specific extras (e.g. an icon reference,
    /// categories, version). Keys are backend-defined; consumers must
    /// treat unknown keys as opaque.
    pub meta: HashMap<String, String>,
}

impl AppEntry {
    /// Bare-bones entry with no display name and no extras.
    pub fn id_only(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: None,
            meta: HashMap::new(),
        }
    }
}

/// A "cast one app to this desktop" backend (adb+scrcpy, ssh-x11, waypipe...).
///
/// Async methods return manually boxed futures so the trait stays usable as
/// `Box<dyn Transporter>` inside [`crate::core::registry::TransporterRegistry`].
pub trait Transporter: Send + Sync {
    /// Registry name of this protocol (e.g. `"adb"`).
    fn name(&self) -> &'static str;

    /// Run one full casting session: verify the target, launch the app on a
    /// virtual display, mirror it, and clean up on any exit path.
    fn run<'a>(&'a self, config: &'a ResolvedConfig) -> BoxFut<'a, Result<(), AppError>>;

    /// List application entries available on `target`
    /// (`pm list packages` / scrcpy app listing for adb, `.desktop` scan
    /// later). Rich fields are best-effort: backends may return
    /// id-only entries when a platform cannot provide more.
    fn list_apps<'a>(
        &'a self,
        target: &'a str,
        params: &'a HashMap<String, String>,
    ) -> BoxFut<'a, Result<Vec<AppEntry>, AppError>>;
}
