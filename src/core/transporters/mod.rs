//! Built-in transporter implementations.

pub mod adb;
pub mod ssh;
pub mod waypipe;

use crate::core::registry::TransporterRegistry;

/// Build the default registry with every statically-compiled backend.
///
/// Dynamic plugins would be injected here in the future (scan dir +
/// `libloading`) without touching any call site.
pub fn default_registry() -> TransporterRegistry {
    let mut registry = TransporterRegistry::new();
    registry.register("adb", || Box::new(adb::AndroidAdbTransporter));
    registry.register("ssh-x11", || Box::new(ssh::LinuxX11Transporter));
    registry.register("waypipe", || Box::new(waypipe::WaylandTransporter));
    registry
}
