//! CLI frontend: clap definitions, the "priority stack" config merge, and
//! subcommand dispatch (`run` / `profile` / `list` / `snapshot`).

use std::collections::HashMap;

use clap::{Args, Parser, Subcommand};
use tracing::debug;

use crate::config::profile::{self, Profile};
use crate::core::error::AppError;
use crate::core::transporter::{resolution_parts, ResolvedConfig};
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
/// Positional `<TRANSPORTER> <TARGET> <APP>` exist *alongside* their
/// long-option twins on purpose: positionals serve the stateless
/// one-liner use case, options serve "tweak a profile" use case, and
/// positionals unconditionally win (ultimate priority).
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

    // ---- dedicated options (priority 2) ----
    /// Override connection protocol
    #[arg(long = "transporter", value_name = "TYPE")]
    pub transporter: Option<String>,
    /// Override target address
    #[arg(long = "target", value_name = "ADDR")]
    pub target: Option<String>,
    /// Override app identifier
    #[arg(long = "app", value_name = "IDENTIFIER")]
    pub app: Option<String>,
    /// Explicit activity (adb only; `.Main` expands to `<pkg>.Main`)
    #[arg(long, value_name = "ACTIVITY")]
    pub activity: Option<String>,
    /// Override resolution as WxH (e.g. 1920x1080)
    #[arg(long, value_name = "WXH")]
    pub resolution: Option<String>,
    /// Override frame rate
    #[arg(long, value_name = "FPS")]
    pub fps: Option<u32>,
    /// Override video bit rate in Mbps
    #[arg(long = "bit-rate", value_name = "MBPS")]
    pub bit_rate: Option<u32>,

    /// Log level: trace|debug|info|error (RUST_LOG still wins)
    #[arg(long = "log-level", value_name = "LEVEL")]
    pub log_level: Option<String>,

    // ---- universal extension params (priority 3) ----
    /// Extra backend param KEY=VALUE; repeatable; overrides profile params
    #[arg(long = "param", value_name = "KEY=VALUE")]
    pub extra_params: Vec<String>,

    /// Load ~/.config/appcast/profiles/<NAME>.yaml first (priority 4/5 source)
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Raw args appended verbatim to the scrcpy command (`-- --video-codec=h265 -x`)
    #[arg(last = true, value_name = "RAW_ARGS")]
    pub raw_args: Vec<String>,
}

/// Arguments for `appcast list`.
#[derive(Debug, Args)]
pub struct ListArgs {
    /// Protocol to query (defaults to adb)
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
        /// Target address
        target: String,
        /// App identifier
        app: String,
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

    // Same merge order as run: explicit option > profile > built-in default.
    let transporter_name = args
        .transporter
        .or_else(|| profile.as_ref().map(|p| p.transporter.clone()))
        .unwrap_or_else(|| "adb".to_string());
    let target = args
        .target
        .or_else(|| profile.as_ref().map(|p| p.target.clone()))
        .ok_or(AppError::MissingTarget)?;
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
                activity: None,
                resolution: profile::default_resolution(),
                fps: profile::default_fps(),
                bit_rate: profile::default_bitrate(),
                params: HashMap::new(),
            };
            let path = profile::save_profile(&name, &new_profile)?;
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
/// 1. positional trio  2. dedicated options  3. `--param`
/// 4. profile standard fields  5. profile params  6. defaults/auto-resolve.
///
/// Activity is intentionally left unresolved here when absent: auto-resolution
/// needs adb round-trips, which belong to the adb backend, not the frontend.
fn merge_config(args: &RunArgs, profile: Option<Profile>) -> Result<ResolvedConfig, AppError> {
    // Explode the profile into an Option-view so every field can participate
    // in its own `.or(...)` chain independently.
    let (p_transporter, p_target, p_app, p_activity, p_resolution, p_fps, p_bitrate, mut params) =
        match profile {
            Some(p) => (
                Some(p.transporter),
                Some(p.target),
                Some(p.app),
                p.activity,
                Some(p.resolution),
                Some(p.fps),
                Some(p.bit_rate),
                p.params,
            ),
            None => (None, None, None, None, None, None, None, HashMap::new()),
        };

    // ---- core trio: three-step or-chain, error when fully empty ----
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
        .or(p_target)
        .ok_or(AppError::MissingTarget)?;

    let app = args
        .positional_app
        .clone()
        .or_else(|| args.app.clone())
        .or(p_app)
        .ok_or(AppError::MissingApp)?;

    // ---- typed fields: option > profile > hardcoded default ----
    let resolution = args
        .resolution
        .clone()
        .or(p_resolution)
        .unwrap_or_else(profile::default_resolution);
    // Fail fast on malformed resolutions before any subprocess is spawned;
    // the actual long-edge extraction happens later inside the adb backend.
    resolution_parts(&resolution)?;

    // ---- extension params: profile params form the base, then each
    //      `--param KEY=VALUE` overrides the same-named key ----
    for kv in &args.extra_params {
        let (key, value) = kv
            .split_once('=')
            .ok_or_else(|| AppError::InvalidParamFormat(kv.clone()))?;
        params.insert(key.to_string(), value.to_string());
    }

    Ok(ResolvedConfig {
        transporter,
        target,
        app,
        activity: args.activity.clone().or(p_activity),
        resolution,
        fps: args.fps.or(p_fps).unwrap_or_else(profile::default_fps),
        bit_rate: args
            .bit_rate
            .or(p_bitrate)
            .unwrap_or_else(profile::default_bitrate),
        params,
        raw_args: args.raw_args.clone(),
    })
}

/// Render the final config back into one copy-pasteable shell line
/// (pure text output — never a script or binary file).
fn render_snapshot(config: &ResolvedConfig) -> String {
    let mut parts = vec![
        "appcast".to_string(),
        "run".to_string(),
        shell_quote(&config.transporter),
        shell_quote(&config.target),
        shell_quote(&config.app),
    ];

    if let Some(activity) = &config.activity {
        parts.push("--activity".into());
        parts.push(shell_quote(activity));
    }
    for (flag, value) in [
        ("--resolution", config.resolution.clone()),
        ("--fps", config.fps.to_string()),
        ("--bit-rate", config.bit_rate.to_string()),
    ] {
        parts.push(flag.into());
        parts.push(value);
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
            target: "old-host".into(),
            app: "/usr/bin/firefox".into(),
            activity: Some(".MainActivity".into()),
            resolution: "1280x720".into(),
            fps: 30,
            bit_rate: 4,
            params: HashMap::from([
                ("port".to_string(), "22".to_string()),
                ("keep".to_string(), "yes".to_string()),
            ]),
        }
    }

    #[test]
    fn positional_beats_option_beats_profile() {
        let mut args = positional("adb", "dev1", "com.a.b");
        args.transporter = Some("waypipe".into()); // must lose to positional

        let config = merge_config(&args, Some(sample_profile())).unwrap();
        assert_eq!(config.transporter, "adb");
        assert_eq!(config.target, "dev1");
        assert_eq!(config.app, "com.a.b");
        // Untouched fields still flow through from lower-priority layers:
        assert_eq!(config.activity.as_deref(), Some(".MainActivity"));
        assert_eq!(config.fps, 30);
    }

    #[test]
    fn option_overrides_profile_without_positional() {
        let args = RunArgs {
            bit_rate: Some(12),
            ..RunArgs::default()
        };

        let profile = sample_profile();
        let config = merge_config(&args, Some(profile)).unwrap();
        assert_eq!(config.transporter, "ssh-x11"); // from profile
        assert_eq!(config.bit_rate, 12); // option beat profile's 4
        assert_eq!(config.resolution, "1280x720"); // untouched profile field
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
    fn dedicated_bit_rate_wins_over_param_alias() {
        let mut args = positional("adb", "d", "p");
        args.bit_rate = Some(4);
        args.extra_params = vec!["bit_rate=8".into()];

        let config = merge_config(&args, None).unwrap();
        // The strongly-typed field wins; the param alias is inert data.
        assert_eq!(config.bit_rate, 4);
    }

    #[test]
    fn defaults_apply_when_nothing_set() {
        let config = merge_config(&positional("adb", "d", "p"), None).unwrap();
        assert_eq!(config.resolution, "1920x1080");
        assert_eq!(config.fps, 60);
        assert_eq!(config.bit_rate, 8);
        assert!(config.activity.is_none()); // backend will auto-resolve
    }

    #[test]
    fn missing_fields_produce_precise_errors() {
        assert!(matches!(
            merge_config(&RunArgs::default(), None),
            Err(AppError::MissingTransporter)
        ));
        let partial = RunArgs {
            positional_transporter: Some("adb".into()),
            ..RunArgs::default()
        };
        assert!(matches!(
            merge_config(&partial, None),
            Err(AppError::MissingTarget)
        ));
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
    fn invalid_resolution_fails_fast() {
        let mut args = positional("adb", "d", "p");
        args.resolution = Some("big".into());
        assert!(matches!(
            merge_config(&args, None),
            Err(AppError::InvalidResolutionFormat(_))
        ));
    }

    #[test]
    fn snapshot_line_round_trips_through_clap() {
        let mut args = positional("adb", "emulator-5554", "com.tencent.mm");
        args.fps = Some(90);
        args.extra_params = vec!["b=2".into(), "a=1".into()];
        args.raw_args = vec!["-W".into()];

        let config = merge_config(&args, None).unwrap();
        let line = render_snapshot(&config);
        assert_eq!(
            line,
            "appcast run adb emulator-5554 com.tencent.mm \
             --resolution 1920x1080 --fps 90 --bit-rate 8 \
             --param a=1 --param b=2 -- -W"
        );
    }

    #[test]
    fn shell_quotes_unsafe_values() {
        assert_eq!(shell_quote("plain-value"), "plain-value");
        assert_eq!(shell_quote("with space"), "'with space'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
