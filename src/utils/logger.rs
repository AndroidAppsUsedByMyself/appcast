//! tracing bootstrap: stderr for humans, daily-rotated file for the record.
//!
//! Stateless-tolerant: if the state dir cannot be created we silently
//! degrade to stderr-only logging instead of failing the command.

use etcetera::base_strategy::{choose_base_strategy, BaseStrategy};
use tracing_subscriber::{
    layer::{Layer as _, SubscriberExt},
    util::SubscriberInitExt,
    EnvFilter,
};

/// Initialize global tracing.
///
/// Directive priority: `RUST_LOG` > explicit `--log-level` > `info`.
pub fn init(level_override: Option<&str>) {
    let directive = std::env::var("RUST_LOG")
        .ok()
        .or_else(|| level_override.map(str::to_owned))
        .unwrap_or_else(|| "info".to_owned());
    let fallback = || EnvFilter::new("info");

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(EnvFilter::try_new(&directive).unwrap_or_else(|_| fallback()));

    let base = tracing_subscriber::registry().with(stderr_layer);

    // Best-effort daily log file under $XDG_STATE_HOME/appcast/logs.
    let file_layer = choose_base_strategy().ok().and_then(|strategy| {
        let dir = strategy
            .state_dir()
            .unwrap_or_else(|| strategy.config_dir())
            .join("appcast")
            .join("logs");
        std::fs::create_dir_all(&dir).ok()?;
        Some(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(tracing_appender::rolling::daily(&dir, "appcast.log"))
                .with_filter(EnvFilter::try_new(&directive).unwrap_or_else(|_| fallback())),
        )
    });

    match file_layer {
        Some(layer) => base.with(layer).init(),
        None => base.init(),
    }
}
