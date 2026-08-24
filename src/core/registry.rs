//! Transporter registry: static registration today, dynamic `.so` loading later.

use std::collections::HashMap;

use crate::core::error::AppError;
use crate::core::transporter::Transporter;

type Factory = Box<dyn Fn() -> Box<dyn Transporter> + Send + Sync>;

/// Maps protocol names (`"adb"`, ...) to factories producing fresh
/// [`Transporter`] instances.
///
/// Because callers only ever see `registry.get(name)`, adding dynamic plugin
/// support later (scan a dir, `libloading`, inject factories) requires zero
/// changes at call sites.
pub struct TransporterRegistry {
    factories: HashMap<String, Factory>,
}

impl TransporterRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    /// Register a factory under `name`; later registration overwrites.
    pub fn register<F>(&mut self, name: &str, factory: F)
    where
        F: Fn() -> Box<dyn Transporter> + Send + Sync + 'static,
    {
        self.factories.insert(name.to_string(), Box::new(factory));
    }

    /// Instantiate the transporter registered under `name`.
    ///
    /// Unknown names carry the live registry contents in the error, so the
    /// built-in list can never drift from reality.
    pub fn get(&self, name: &str) -> Result<Box<dyn Transporter>, AppError> {
        self.factories.get(name).map(|factory| factory()).ok_or_else(|| {
            let mut available: Vec<&str> = self.factories.keys().map(String::as_str).collect();
            available.sort_unstable();
            AppError::UnknownTransporter {
                name: name.to_string(),
                available: available.join(", "),
            }
        })
    }
}

impl Default for TransporterRegistry {
    fn default() -> Self {
        Self::new()
    }
}
