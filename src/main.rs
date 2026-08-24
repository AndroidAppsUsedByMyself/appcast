//! `appcast` — cast a single application's GUI into a native window on this
//! desktop, via pluggable transporters (adb/scrcpy today; ssh-x11/waypipe later).
//!
//! Layering: [`cli`] (frontend) → [`core`] (transporter trait + registry) →
//! concrete backends. The `cli` module only exists when the `cli` feature is
//! enabled (default); TUI/WebUI frontends are reserved behind cargo features.

#[cfg(feature = "cli")]
mod cli;
#[cfg(feature = "cli")]
mod config;
#[cfg(feature = "cli")]
mod core;
#[cfg(feature = "cli")]
mod utils;

fn main() -> std::process::ExitCode {
    #[cfg(feature = "cli")]
    {
        // Hand-rolled runtime instead of #[tokio::main] so we can render the
        // error chain with `{err:#}` and control the exit code precisely.
        match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
            Ok(rt) => match rt.block_on(cli::run_cli()) {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(err) => {
                    cli::report_error(&err);
                    std::process::ExitCode::FAILURE
                }
            },
            Err(err) => {
                eprintln!("error: failed to bootstrap async runtime: {err}");
                std::process::ExitCode::FAILURE
            }
        }
    }

    #[cfg(not(feature = "cli"))]
    {
        eprintln!("error: no frontend compiled; build with `--features cli`");
        std::process::ExitCode::FAILURE
    }
}
