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

    /// List application identifiers available on `target`
    /// (`pm list packages` for adb, `.desktop` scan later).
    fn list_apps<'a>(
        &'a self,
        target: &'a str,
        params: &'a HashMap<String, String>,
    ) -> BoxFut<'a, Result<Vec<String>, AppError>>;
}
