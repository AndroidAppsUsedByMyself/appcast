//! Unified application error type (`thiserror`).

use thiserror::Error;

/// Every recoverable failure the CLI can surface to the user.
#[derive(Debug, Error)]
pub enum AppError {
    #[error(
        "missing transporter: pass it positionally (`appcast run adb-scrcpy <target> <app>`), \
         or via --transporter / --profile"
    )]
    MissingTransporter,

    /// Authored by the backend that misses it — the core does not know which
    /// addressing slots each transporter requires.
    #[error("usage: {0}")]
    Usage(String),

    #[error(
        "scrcpy >= {required} required for virtual-display support (found {found}); \
         please upgrade scrcpy"
    )]
    ScrcpyTooOld { found: u32, required: u32 },

    #[error("cannot determine scrcpy version: {0}")]
    ScrcpyVersionUnknown(String),

    #[error("unknown transporter `{name}` (built-in: {available})")]
    UnknownTransporter {
        name: String,
        available: String,
    },

    #[error("device not found or unreachable: {0}")]
    DeviceNotFound(String),

    #[error(
        "invalid app identifier `{0}`: for the adb-scrcpy backend this must be an Android \
         package name such as `com.example.app`, not a path"
    )]
    InvalidAppIdentifier(String),

    #[error("failed to spawn scrcpy: {0} (is scrcpy installed and on PATH?)")]
    ScrcpySpawnFailed(String),

    #[error(
        "invalid URL `{0}`: the web-browser backend expects an http(s) URL \
         such as `https://example.com`"
    )]
    InvalidUrl(String),

    #[error(
        "no supported browser found (tried: {tried}); \
         install one, or point at yours via --param browser_path=<path>"
    )]
    NoBrowserFound { tried: String },

    #[error("failed to launch browser `{0}` (is browser_path correct?)")]
    BrowserLaunchFailed(String),

    #[error("profile `{0}` not found under ~/.config/appcast/profiles/")]
    ProfileNotFound(String),

    #[error("invalid resolution `{0}`: expected `<W>x<H>`, e.g. `1920x1080`")]
    InvalidResolutionFormat(String),

    #[error("invalid --param `{0}`: expected KEY=VALUE")]
    InvalidParamFormat(String),

    #[error("invalid value for param `{key}`: `{value}`")]
    InvalidParamValue { key: String, value: String },

    #[error("backend error: {0}")]
    BackendError(String),

    #[error("`{0}` is not implemented yet")]
    NotImplemented(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Yaml(#[from] serde_yaml::Error),
}
