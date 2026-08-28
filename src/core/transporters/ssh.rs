//! Placeholder backend: remote Linux X11 app forwarded over SSH (`ssh -X`).

use std::collections::HashMap;

use crate::core::error::AppError;
use crate::core::transporter::{AppEntry, BoxFut, ResolvedConfig, Transporter};

/// SSH + X11 forwarding backend (not implemented yet).
pub struct LinuxX11Transporter;

impl Transporter for LinuxX11Transporter {
    fn name(&self) -> &'static str {
        "ssh-x11"
    }

    fn description(&self) -> &'static str {
        "Remote Linux X11 app forwarded over SSH (not yet implemented)"
    }

    fn run<'a>(&'a self, _config: &'a ResolvedConfig) -> BoxFut<'a, Result<(), AppError>> {
        Box::pin(async { Err(AppError::NotImplemented("ssh-x11 backend".into())) })
    }

    fn list_apps<'a>(
        &'a self,
        _target: &'a str,
        _params: &'a HashMap<String, String>,
    ) -> BoxFut<'a, Result<Vec<AppEntry>, AppError>> {
        Box::pin(async { Err(AppError::NotImplemented("ssh-x11 backend".into())) })
    }
}
