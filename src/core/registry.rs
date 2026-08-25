//! Transporter registry: static registration plus runtime plugin loading.
//!
//! Two kinds of entries share one namespace:
//! - **Built-in backends** register factories (cheap stateless structs, a
//!   fresh instance per `get`).
//! - **Plugins** (`.so`/`.dylib`/`.dll` loaded by
//!   [`crate::core::plugins`]) register a ready-made instance behind an
//!   [`Arc`]: the loaded library cannot be cheaply re-instantiated per
//!   call, and cloning an [`Arc`] is all a fresh `get` needs.
//!
//! Later registration overwrites earlier ones, so a plugin may replace a
//! built-in backend under the same name — forks patch without renaming.
//! Because callers only ever see `registry.get(name)`, swapping either
//! mechanism for another requires zero changes at call sites.

use std::path::PathBuf;
use std::sync::Arc;

use crate::core::error::AppError;
use crate::core::transporter::Transporter;

type Factory = Box<dyn Fn() -> Box<dyn Transporter> + Send + Sync>;

/// How an entry produces transporter instances on demand.
enum Entry {
    /// Statically compiled backend: fresh (stateless) instance per get().
    Factory(Factory),
    /// Ready-made shared instance — dynamic plugins.
    Ready(Arc<dyn Transporter>),
}

/// Where a registration came from, surfaced by `appcast transporters`.
#[derive(Clone, Debug)]
pub enum Origin {
    /// Compiled into this binary via `default_registry`.
    BuiltIn,
    /// Loaded from a plugin file at startup; carries its full path so
    /// users can tell which artifact is active.
    Plugin(PathBuf),
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Origin::BuiltIn => write!(f, "built-in"),
            Origin::Plugin(path) => write!(f, "{}", path.display()),
        }
    }
}

struct Registered {
    entry: Entry,
    origin: Origin,
}

/// Maps protocol names (`"adb-scrcpy"`, ...) to transporter sources.
pub struct TransporterRegistry {
    registrations: std::collections::HashMap<String, Registered>,
}

impl TransporterRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            registrations: std::collections::HashMap::new(),
        }
    }

    /// Register a built-in factory under `name`; later registration
    /// overwrites.
    pub fn register<F>(&mut self, name: &str, factory: F)
    where
        F: Fn() -> Box<dyn Transporter> + Send + Sync + 'static,
    {
        self.registrations.insert(
            name.to_string(),
            Registered {
                entry: Entry::Factory(Box::new(factory)),
                origin: Origin::BuiltIn,
            },
        );
    }

    /// Register a ready-made instance (dynamic plugin) with its origin
    /// path; later registration overwrites.
    pub fn register_plugin(
        &mut self,
        name: &str,
        instance: Arc<dyn Transporter>,
        path: PathBuf,
    ) {
        self.registrations.insert(
            name.to_string(),
            Registered {
                entry: Entry::Ready(instance),
                origin: Origin::Plugin(path),
            },
        );
    }

    /// All registered protocol names, sorted.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.registrations.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// Every registration as `(name, origin)`, sorted by name — the data
    /// behind the `transporters` command's provenance listing.
    pub fn entries(&self) -> Vec<(String, Origin)> {
        let mut entries: Vec<(String, Origin)> = self
            .registrations
            .iter()
            .map(|(name, reg)| (name.clone(), reg.origin.clone()))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
    }

    /// Instantiate the transporter registered under `name`, shared behind
    /// an [`Arc`].
    ///
    /// Unknown names carry the live registry contents in the error, so the
    /// installed list can never drift from reality.
    pub fn get(&self, name: &str) -> Result<Arc<dyn Transporter>, AppError> {
        let registered = self.registrations.get(name).ok_or_else(|| {
            let mut available: Vec<&str> =
                self.registrations.keys().map(String::as_str).collect();
            available.sort_unstable();
            AppError::UnknownTransporter {
                name: name.to_string(),
                available: available.join(", "),
            }
        })?;
        Ok(match &registered.entry {
            Entry::Factory(factory) => Arc::from(factory()),
            Entry::Ready(instance) => Arc::clone(instance),
        })
    }
}

impl Default for TransporterRegistry {
    fn default() -> Self {
        Self::new()
    }
}
