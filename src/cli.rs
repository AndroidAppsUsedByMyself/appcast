//! CLI frontend: clap definitions, the "priority stack" config merge, and
//! subcommand dispatch (`run` / `profile` / `list` / `snapshot`).

use std::collections::HashMap;

use clap::{Args, Parser, Subcommand};
use tracing::debug;

use crate::config::profile::{self, Profile};
use crate::core::error::AppError;
use crate::core::transporter::ResolvedConfig;
use crate::core::transporters;
use crate::utils::logger;

/// Top-level `appcast` command.
#[derive(Debug, Parser)]
#[command(
    name = "appcast",
    version,
    about = "Cast a remote/local app's screen into a native window on this desktop"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// All subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Cast an app to this desktop now.
    Run(RunArgs),
    /// Manage saved profiles (save/list/edit/rm).
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
    /// List apps available on a target.
    List(ListArgs),
    /// Print the fully merged command line without executing anything.
    Snapshot(RunArgs),
}

/// Shared argument surface of `run` and `snapshot`.
///
/// The CLI surface stays deliberately thin: the universal addressing trio
/// (`<TRANSPORTER> <TARGET> <APP>` plus their `--` twins) and two opaque
/// channels (`--param`, `--`). Everything backend-specific travels as
/// params — backends interpret them and own their defaults.
#[derive(Debug, Default, Args)]
pub struct RunArgs {
    // ---- positional trio (priority 1) ----
    /// Connection protocol (adb | ssh-x11 | waypipe)
    #[arg(value_name = "TRANSPORTER")]
    pub positional_transporter: Option<String>,
    /// Target address (device serial / host)
    #[arg(value_name = "TARGET")]
    pub positional_target: Option<String>,
    /// App identifier (Android package / executable path)
    #[arg(value_name = "APP")]
    pub positional_app: Option<String>,

    // ---- dedicated trio options (priority 2) ----
    /// Override connection protocol
    #[arg(long = "transporter", value_name = "TYPE")]
    pub transporter: Option<String>,
    /// Override target address
    #[arg(long = "target", value_name = "ADDR")]
    pub target: Option<String>,
    /// Override app identifier
    #[arg(long = "app", value_name = "IDENTIFIER")]
    pub app: Option<String>,

    /// Log level: trace|debug|info|error (RUST_LOG still wins)
    #[arg(long = "log-level", value_name = "LEVEL")]
    pub log_level: Option<String>,

    // ---- backend params (priority 3) ----
    /// Backend param KEY=VALUE; repeatable; overrides profile params.
    /// adb/scrcpy knows: resolution, fps, bit_rate, adb_path, scrcpy_path
    #[arg(long = "param", value_name = "KEY=VALUE")]
    pub extra_params: Vec<String>,

    /// Load ~/.config/appcast/profiles/<NAME>.yaml first (priority 4/5 source)
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Raw args appended verbatim to the scrcpy command; overrides the
    /// profile's `raw_args` when non-empty (`-- --video-codec=h265 -x`)
    #[arg(last = true, value_name = "RAW_ARGS")]
    pub raw_args: Vec<String>,
}

/// Arguments for `appcast list`.
#[derive(Debug, Args)]
pub struct ListArgs {
    /// Transporter whose listing semantics to use (e.g. adb-scrcpy)
    #[arg(value_name = "TRANSPORTER")]
    pub positional_transporter: Option<String>,
    /// Override transporter (--transporter wins over the positional)
    #[arg(long = "transporter", value_name = "TYPE")]
    pub transporter: Option<String>,
    /// Target address (required unless present in --profile)
    #[arg(long = "target", value_name = "ADDR")]
    pub target: Option<String>,
    /// Source transporter/target/params from this profile
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,
}

/// Subcommands of `appcast profile`.
#[derive(Debug, Subcommand)]
pub enum ProfileAction {
    /// Save arguments as a profile (overwrites an existing one).
    Save {
        /// Profile name
        name: String,
        /// Connection protocol
        transporter: String,
        /// Address slot (required by most transporters)
        target: Option<String>,
        /// Content slot (optional, per transporter)
        app: Option<String>,
    },
    /// List saved profiles.
    List,
    /// Open the profile YAML in $EDITOR (creates a template if missing).
    Edit {
        /// Profile name
        name: String,
    },
    /// Delete a profile.
    Rm {
        /// Profile name
        name: String,
    },
}

/// Parse argv, init logging, dispatch. The single entry point from `main`.
pub async fn run_cli() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Logging needs the resolved level; file writes degrade silently on error.
    let level = match &cli.command {
        Command::Run(args) | Command::Snapshot(args) => args.log_level.as_deref(),
        _ => None,
    };
    logger::init(level);

    match cli.command {
        Command::Run(args) => cmd_run(args).await,
        Command::Snapshot(args) => cmd_snapshot(args).await,
        Command::List(args) => cmd_list(args).await,
        Command::Profile { action } => cmd_profile(action).await,
    }
}

async fn cmd_run(args: RunArgs) -> anyhow::Result<()> {
    let profile = load_optional_profile(args.profile.as_deref())?;
    let config = merge_config(&args, profile)?;

    let transporter = transporters::default_registry().get(&config.transporter)?;
    debug!(transporter = transporter.name(), "dispatching to backend");
    transporter.run(&config).await?;
    Ok(())
}

async fn cmd_snapshot(args: RunArgs) -> anyhow::Result<()> {
    let profile = load_optional_profile(args.profile.as_deref())?;
    let config = merge_config(&args, profile)?;
    println!("{}", render_snapshot(&config));
    Ok(())
}

async fn cmd_list(args: ListArgs) -> anyhow::Result<()> {
    let profile = load_optional_profile(args.profile.as_deref())?;

    // Listing is backend-specific (pm list vs .desktop scan vs ...), so the
    // transporter is required here too — same explicitness as `run`.
    const USAGE: &str = "appcast list <TRANSPORTER> --target <ADDR>";
    let transporter_name = args
        .positional_transporter
        .clone()
        .or(args.transporter)
        .or_else(|| profile.as_ref().map(|p| p.transporter.clone()))
        .ok_or(AppError::Usage(USAGE.into()))?;
    let target = args
        .target
        .or_else(|| profile.as_ref().and_then(|p| p.target.clone()))
        .ok_or(AppError::Usage(USAGE.into()))?;
    let params = profile.map(|p| p.params).unwrap_or_default();

    let transporter = transporters::default_registry().get(&transporter_name)?;
    for app in transporter.list_apps(&target, &params).await? {
        println!("{app}");
    }
    Ok(())
}

async fn cmd_profile(action: ProfileAction) -> anyhow::Result<()> {
    match action {
        ProfileAction::Save {
            name,
            transporter,
            target,
            app,
        } => {
            let new_profile = Profile {
                transporter,
                target,
                app,
                params: HashMap::new(),
                raw_args: Vec::new(),
            };            let path = profile::save_profile(&name, &new_profile)?;
            println!("saved profile `{name}` → {}", path.display());
        }
        ProfileAction::List => {
            for name in profile::list_profiles()? {
                println!("{name}");
            }
        }
        ProfileAction::Edit { name } => profile::edit_profile(&name).await?,
        ProfileAction::Rm { name } => {
            profile::delete_profile(&name)?;
            println!("removed profile `{name}`");
        }
    }
    Ok(())
}

/// Only touch the filesystem when `--profile` was actually passed
/// (stateless tolerance): no flag → never read any config file.
fn load_optional_profile(name: Option<&str>) -> Result<Option<Profile>, AppError> {
    match name {
        Some(name) => Ok(Some(profile::load_profile(name)?)),
        None => Ok(None),
    }
}

/// Priority-stack merge (highest → lowest):
/// 1. positional slots  2. dedicated slot options (`--transporter/--target/--app`)
/// 3. `--param` overrides (per-key)  4. profile fields.
///
/// The core never validates addressing arity: `target`/`app` stay `Option`
/// and each backend rejects what it cannot work with, using its own usage
/// text. Display knobs (resolution/fps/bit_rate) are plain params,
/// interpreted — with defaults — by the selected backend.
fn merge_config(args: &RunArgs, profile: Option<Profile>) -> Result<ResolvedConfig, AppError> {
    // Explode the profile into an Option-view so every field can participate
    // in its own `.or(...)` chain independently.
    let (p_transporter, p_target, p_app, mut params, p_raw_args) = match profile {
        Some(p) => (Some(p.transporter), p.target, p.app, p.params, p.raw_args),
        None => (None, None, None, HashMap::new(), Vec::new()),
    };

    // ---- addressing slots: pure or-chain; arity validation belongs to the
    //      selected backend, which owns its own usage message ----
    let transporter = args
        .positional_transporter
        .clone()
        .or_else(|| args.transporter.clone())
        .or(p_transporter)
        .ok_or(AppError::MissingTransporter)?;

    let target = args
        .positional_target
        .clone()
        .or_else(|| args.target.clone())
        .or(p_target);

    let app = args
        .positional_app
        .clone()
        .or_else(|| args.app.clone())
        .or(p_app);

    // ---- extension params: profile params form the base, then each
    //      `--param KEY=VALUE` overrides the same-named key ----
    for kv in &args.extra_params {
        let (key, value) = kv
            .split_once('=')
            .ok_or_else(|| AppError::InvalidParamFormat(kv.clone()))?;
        params.insert(key.to_string(), value.to_string());
    }

    // ---- raw passthrough: profile provides a base, but any CLI `--` args
    //      replace it wholesale (same "explicit wins" rule as scalars) ----
    let raw_args = if args.raw_args.is_empty() {
        p_raw_args
    } else {
        args.raw_args.clone()
    };

    Ok(ResolvedConfig {
        transporter,
        target,
        app,
        params,
        raw_args,
    })
}

/// Render the final config back into one copy-pasteable shell line
/// (pure text output — never a script or binary file).
fn render_snapshot(config: &ResolvedConfig) -> String {
    let mut parts = vec!["appcast".to_string(), "run".to_string()];
    parts.push(shell_quote(&config.transporter));
    if let Some(target) = &config.target {
        parts.push(shell_quote(target));
    }
    if let Some(app) = &config.app {
        parts.push(shell_quote(app));
    }

    // Deterministic ordering makes snapshots diff-friendly.
    let mut keys: Vec<&String> = config.params.keys().collect();
    keys.sort();
    for key in keys {
        parts.push("--param".into());
        parts.push(format!("{key}={}", config.params[key]));
    }

    if !config.raw_args.is_empty() {
        parts.push("--".into());
        parts.extend(config.raw_args.iter().map(|arg| shell_quote(arg)));
    }

    parts.join(" ")
}

/// Minimal shell quoting: conservative charset passes through bare,
/// everything else gets single-quoted POSIX-style.
fn shell_quote(value: &str) -> String {
    let safe = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-:@+=,:%".contains(c));
    if safe {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positional(t: &str, target: &str, app: &str) -> RunArgs {
        RunArgs {
            positional_transporter: Some(t.into()),
            positional_target: Some(target.into()),
            positional_app: Some(app.into()),
            ..RunArgs::default()
        }
    }

    fn sample_profile() -> Profile {
        Profile {
            transporter: "ssh-x11".into(),
            target: Some("old-host".into()),
            app: Some("/usr/bin/firefox".into()),
            params: HashMap::from([
                ("port".to_string(), "22".to_string()),
                ("keep".to_string(), "yes".to_string()),
                ("fps".to_string(), "30".to_string()),
            ]),
            raw_args: vec!["--keep-active".to_string()],
        }
    }

    #[test]
    fn positional_beats_option_beats_profile() {
        let mut args = positional("adb", "dev1", "com.a.b");
        args.transporter = Some("waypipe".into()); // must lose to positional

        let config = merge_config(&args, Some(sample_profile())).unwrap();
        assert_eq!(config.transporter, "adb");
        assert_eq!(config.target.as_deref(), Some("dev1"));
        assert_eq!(config.app.as_deref(), Some("com.a.b"));
        // Untouched fields still flow through from lower-priority layers:
        assert_eq!(config.param("port"), Some("22"));
        assert_eq!(config.raw_args, vec!["--keep-active"]);
    }

    #[test]
    fn profile_fills_everything_when_cli_is_bare() {
        let args = RunArgs::default();

        let config = merge_config(&args, Some(sample_profile())).unwrap();
        assert_eq!(config.transporter, "ssh-x11"); // from profile
        assert_eq!(config.param("fps"), Some("30")); // backend knob via params
    }

    #[test]
    fn slots_may_stay_absent_for_slotless_backends() {
        // e.g. a web transporter needs only (transporter, target=url);
        // the core must not reject the missing app slot — arity is the
        // backend's business.
        let args = RunArgs {
            positional_transporter: Some("web".into()),
            positional_target: Some("https://example.com".into()),
            ..RunArgs::default()
        };
        let config = merge_config(&args, None).unwrap();
        assert_eq!(config.target.as_deref(), Some("https://example.com"));
        assert!(config.app.is_none());
    }

    #[test]
    fn missing_transporter_is_the_only_core_level_error() {
        assert!(matches!(
            merge_config(&RunArgs::default(), None),
            Err(AppError::MissingTransporter)
        ));
    }

    #[test]
    fn param_overrides_profile_param_only() {
        let mut args = positional("adb", "d", "p");
        args.extra_params = vec!["port=2222".into()];
        let mut profile = sample_profile();
        profile.params.insert("port".into(), "22".into());

        let config = merge_config(&args, Some(profile)).unwrap();
        assert_eq!(config.param("port"), Some("2222")); // overridden
        assert_eq!(config.param("keep"), Some("yes")); // preserved
    }

    #[test]
    fn invalid_param_format_is_rejected() {
        let mut args = positional("adb", "d", "p");
        args.extra_params = vec!["no-equals-sign".into()];
        assert!(matches!(
            merge_config(&args, None),
            Err(AppError::InvalidParamFormat(_))
        ));
    }

    #[test]
    fn raw_args_profile_fallback_and_cli_override() {
        // No CLI passthrough → profile's raw_args flow through.
        let args = positional("adb", "d", "p");
        let config = merge_config(&args, Some(sample_profile())).unwrap();
        assert_eq!(config.raw_args, vec!["--keep-active"]);

        // Non-empty CLI passthrough replaces the profile's wholesale.
        let mut override_args = positional("adb", "d", "p");
        override_args.raw_args = vec!["-x".into()];
        let config = merge_config(&override_args, Some(sample_profile())).unwrap();
        assert_eq!(config.raw_args, vec!["-x"]);
    }

    #[test]
    fn snapshot_line_round_trips_through_clap() {
        let mut args = positional("adb", "emulator-5554", "com.tencent.mm");
        args.extra_params = vec!["b=2".into(), "a=1".into(), ("fps=90").into()];
        args.raw_args = vec!["-W".into()];

        let config = merge_config(&args, None).unwrap();
        let line = render_snapshot(&config);
        assert_eq!(
            line,
            "appcast run adb emulator-5554 com.tencent.mm \
             --param a=1 --param b=2 --param fps=90 -- -W"
        );
    }

    #[test]
    fn shell_quotes_unsafe_values() {
        assert_eq!(shell_quote("plain-value"), "plain-value");
        assert_eq!(shell_quote("with space"), "'with space'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
