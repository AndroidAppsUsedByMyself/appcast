//! Built-in transporter implementations.
//!
//! Every backend sits behind its own cargo feature (`adapter-adb`,
//! `adapter-browser`, ...) so minimal builds can drop both the code and
//! the adapter's exclusive dependencies. Registering is likewise
//! feature-gated: a disabled adapter simply vanishes from the registry,
//! which in turn removes it from CLI validation, suggestions and shell
//! completions — no other call site knows or cares.

#[cfg(feature = "adapter-adb")]
pub mod adb;
#[cfg(feature = "adapter-browser")]
pub mod browser;
pub mod session;
#[cfg(feature = "adapter-ssh")]
pub mod ssh;
#[cfg(feature = "adapter-waypipe")]
pub mod waypipe;

use crate::core::registry::TransporterRegistry;

/// Build the default registry with every statically-compiled backend.
///
/// Names encode content + window mechanism (`adb-scrcpy` = adb control
/// channel + scrcpy virtual display, `web-browser` = web app cast through
/// a system browser) so that alternative pipelines for the same platform
/// (e.g. a fork's `adb-amstart`) never collide.
pub fn default_registry() -> TransporterRegistry {
    let mut registry = TransporterRegistry::new();
    #[cfg(feature = "adapter-adb")]
    registry.register("adb-scrcpy", || Box::new(adb::AdbScrcpyTransporter));
    #[cfg(feature = "adapter-browser")]
    registry.register("web-browser", || Box::new(browser::WebBrowserTransporter));
    #[cfg(feature = "adapter-ssh")]
    registry.register("ssh-x11", || Box::new(ssh::LinuxX11Transporter));
    #[cfg(feature = "adapter-waypipe")]
    registry.register("waypipe", || Box::new(waypipe::WaylandTransporter));
    registry
}

/// The registry the CLI actually uses: built-ins first, then plugins from
/// the configured search dirs (which may override same-named built-ins).
/// Every call site funnels through here so plugin discovery happens exactly
/// once per invocation.
pub fn build_registry() -> TransporterRegistry {
    let mut registry = default_registry();
    crate::core::plugins::load_into(&mut registry);
    registry
}
