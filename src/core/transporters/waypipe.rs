//! Placeholder backend: remote Wayland app forwarded via waypipe.

use std::collections::HashMap;

use crate::core::error::AppError;
use crate::core::transporter::{AppEntry, BoxFut, ResolvedConfig, Transporter};

/// Wayland / waypipe backend (not implemented yet).
pub struct WaylandTransporter;

impl Transporter for WaylandTransporter {
    fn name(&self) -> &'static str {
        "waypipe"
    }

    fn description(&self) -> &'static str {
        "Remote Wayland app forwarded via waypipe (not yet implemented)"
    }

    fn run<'a>(&'a self, _config: &'a ResolvedConfig) -> BoxFut<'a, Result<(), AppError>> {
        Box::pin(async { Err(AppError::NotImplemented("waypipe backend".into())) })
    }

    fn list_apps<'a>(
        &'a self,
        _target: &'a str,
        _params: &'a HashMap<String, String>,
    ) -> BoxFut<'a, Result<Vec<AppEntry>, AppError>> {
        Box::pin(async { Err(AppError::NotImplemented("waypipe backend".into())) })
    }
}
