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
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    /// Protocol name, e.g. `"adb"`.
    pub transporter: String,
    /// Device serial / host address.
    pub target: String,
    /// Backend-specific app identifier (Android package name, executable path...).
    pub app: String,
    /// Explicit activity component; `None` lets backends auto-resolve.
    pub activity: Option<String>,
    /// Target resolution as `"WxH"` (e.g. `"1920x1080"`).
    pub resolution: String,
    /// Frame rate.
    pub fps: u32,
    /// Video bit rate in Mbps.
    pub bit_rate: u32,
    /// Free-form extension params (`adb_path`, ssh port, waypipe compression...).
    pub params: HashMap<String, String>,
    /// Raw args to append verbatim to the backend launch command (`-- ...`).
    pub raw_args: Vec<String>,
}

impl ResolvedConfig {
    /// Look up an extension param by key.
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(String::as_str)
    }
}

/// Parse a `"WxH"` string into its `(width, height)` components.
///
/// # Errors
/// [`AppError::InvalidResolutionFormat`] when the input is not `<u32>x<u32>`.
pub fn resolution_parts(resolution: &str) -> Result<(u32, u32), AppError> {
    let invalid = || AppError::InvalidResolutionFormat(resolution.to_string());
    let (w, h) = resolution.split_once('x').ok_or_else(invalid)?;
    let w: u32 = w.trim().parse().map_err(|_| invalid())?;
    let h: u32 = h.trim().parse().map_err(|_| invalid())?;
    Ok((w, h))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_resolution_into_parts() {
        assert_eq!(resolution_parts("1920x1080").unwrap(), (1920, 1080));
        assert_eq!(resolution_parts("1080x1920").unwrap(), (1080, 1920));
    }

    #[test]
    fn malformed_resolution_is_rejected() {
        assert!(matches!(
            resolution_parts("abc"),
            Err(AppError::InvalidResolutionFormat(_))
        ));
        assert!(matches!(
            resolution_parts("1920"),
            Err(AppError::InvalidResolutionFormat(_))
        ));
        assert!(matches!(
            resolution_parts("ax1080"),
            Err(AppError::InvalidResolutionFormat(_))
        ));
    }
}
