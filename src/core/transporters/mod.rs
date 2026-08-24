//! Built-in transporter implementations.

pub mod adb;
pub mod ssh;
pub mod waypipe;

use crate::core::registry::TransporterRegistry;

/// Build the default registry with every statically-compiled backend.
///
/// Names encode the pipeline technique (`adb-scrcpy` = adb control channel +
/// scrcpy virtual display) so that alternative pipelines for the same
/// platform (e.g. a fork's `adb-amstart`) never collide.
pub fn default_registry() -> TransporterRegistry {
    let mut registry = TransporterRegistry::new();
    registry.register("adb-scrcpy", || Box::new(adb::AdbScrcpyTransporter));
    registry.register("ssh-x11", || Box::new(ssh::LinuxX11Transporter));
    registry.register("waypipe", || Box::new(waypipe::WaylandTransporter));
    registry
}
