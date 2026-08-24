//! Unified application error type (`thiserror`).

use thiserror::Error;

/// Every recoverable failure the CLI can surface to the user.
#[derive(Debug, Error)]
pub enum AppError {
    #[error(
        "missing transporter: pass it positionally (`appcast run adb <target> <app>`), \
         or via --transporter / --profile"
    )]
    MissingTransporter,

    #[error("missing target: pass it positionally after the transporter, or via --target / --profile")]
    MissingTarget,

    #[error("missing app identifier: pass it positionally, or via --app / --profile")]
    MissingApp,

    #[error("unknown transporter `{0}` (built-in: adb, ssh-x11, waypipe)")]
    UnknownTransporter(String),

    #[error("device not found or unreachable: {0}")]
    DeviceNotFound(String),

    #[error(
        "invalid app identifier `{0}`: for the adb backend this must be an Android package \
         name such as `com.example.app`, not a path"
    )]
    InvalidAppIdentifier(String),

    #[error("failed to spawn scrcpy: {0} (is scrcpy installed and on PATH?)")]
    ScrcpySpawnFailed(String),

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
