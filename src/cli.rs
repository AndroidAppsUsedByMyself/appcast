//! CLI frontend: clap definitions, the "priority stack" config merge, and
//! subcommand dispatch (`run` / `profile` / `list` / `snapshot`).

use std::collections::HashMap;

use clap::builder::TypedValueParser;
use clap::{Args, CommandFactory, Parser, Subcommand};
use tracing::debug;

use crate::config::profile::{self, Profile};
use crate::core::error::AppError;
use crate::core::transporter::{AppEntry, ResolvedConfig};
use crate::core::transporters;
use crate::utils::logger;

/// Validates transporter names against the live registry at *parse* time,
/// so typos surface with clap-native diagnostics (possible values + a
/// closest-match hint) instead of mid-run backend errors. The candidate
/// list comes from the registry itself, so forked backends join the check
/// (and shell completions) automatically.
#[derive(Clone, Copy, Default)]
struct TransporterValueParser;

impl TypedValueParser for TransporterValueParser {
    type Value = String;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::builder::Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let value = value.to_string_lossy().into_owned();

        let registry = transporters::build_registry();
        let names = registry.names();
        if names.iter().any(|name| *name == value) {
            return Ok(value);
        }

        Err(unknown_transporter_error(cmd, arg, &value, &names))
    }
}

/// Build a clap-native invalid-value error listing every registered
/// transporter (plus a closest-match hint when one is close enough).
fn unknown_transporter_error(
    cmd: &clap::Command,
    arg: Option<&clap::builder::Arg>,
    value: &str,
    names: &[&str],
) -> clap::Error {
    let target = arg.map(|a| a.to_string()).unwrap_or_else(|| "<TRANSPORTER>".into());
    let mut message = format!("invalid value '{value}' for '{target}'");
    message.push_str(&format!("\n  [possible values: {}]", names.join(", ")));
    if let Some(best) = closest_name(value, names) {
        message.push_str(&format!("\n\n  tip: a similar value exists: '{best}'"));
    }
    // Command::error takes &mut self, so work on an error-path-only clone.
    cmd.clone()
        .error(clap::error::ErrorKind::InvalidValue, message)
}

/// Closest registry name within a small edit distance or by prefix,
/// for the "did you mean" hint.
fn closest_name<'a>(input: &str, names: &[&'a str]) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for name in names {
        let distance = edit_distance(input, name);
        let close = distance <= 2 || name.starts_with(input);
        if close && best.is_none_or(|(best_distance, _)| distance < best_distance) {
            best = Some((distance, name));
        }
    }
    best.map(|(_, name)| name)
}

/// Classic two-row Levenshtein distance.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

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
    /// List installed transporters (built-in backends and plugins).
    Transporters,
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
    /// Connection protocol (adb-scrcpy | ssh-x11 | waypipe)
    #[arg(value_name = "TRANSPORTER", value_parser = TransporterValueParser)]
    pub positional_transporter: Option<String>,
    /// Target address (device serial / host)
    #[arg(value_name = "TARGET")]
    pub positional_target: Option<String>,
    /// App identifier (Android package / executable path)
    #[arg(value_name = "APP")]
    pub positional_app: Option<String>,

    // ---- dedicated trio options (priority 2) ----
    /// Override connection protocol
    #[arg(long = "transporter", value_name = "TYPE", value_parser = TransporterValueParser)]
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

    /// Raw args APPENDED to the profile's `raw_args`; use --clear-raw to
    /// start from an empty list instead (`-- --video-codec=h265 -x`)
    #[arg(last = true, value_name = "RAW_ARGS")]
    pub raw_args: Vec<String>,
    /// Ignore the profile's raw_args (CLI passthrough becomes the whole list)
    #[arg(long)]
    pub clear_raw: bool,
}

/// Arguments for `appcast list`.
///
/// Mirrors `run`'s shape minus the content slot: `<TRANSPORTER> [<TARGET>]`.
#[derive(Debug, Args)]
pub struct ListArgs {
    /// Transporter whose listing semantics to use (e.g. adb-scrcpy)
    #[arg(value_name = "TRANSPORTER", value_parser = TransporterValueParser)]
    pub positional_transporter: Option<String>,
    /// Address slot ("where"), same meaning as in `run`
    #[arg(value_name = "TARGET")]
    pub positional_target: Option<String>,
    /// Override transporter (--transporter wins over the positional)
    #[arg(long = "transporter", value_name = "TYPE", value_parser = TransporterValueParser)]
    pub transporter: Option<String>,
    /// Override address slot (--target wins over the positional)
    #[arg(long = "target", value_name = "ADDR")]
    pub target: Option<String>,
    /// Source transporter/target/params from this profile
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,
    /// Show display names alongside ids when the backend provides them
    #[arg(short = 'l', long)]
    pub long: bool,
    /// Emit entries as pretty JSON (stable shape for scripts and UIs)
    #[arg(long)]
    pub json: bool,
}

/// Subcommands of `appcast profile`.
///
/// Save carries the full run-like argument surface inline by design: the
/// enum is built once per invocation and destructured immediately, so the
/// size imbalance costs nothing — boxing would only obscure the fields.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub enum ProfileAction {
    /// Save arguments as a profile (overwrites an existing one).
    ///
    /// Accepts everything a run consumes except --profile itself: the
    /// addressing slots plus --param/-- passthrough, so a saved profile
    /// reproduces the full configuration.
    Save {
        /// Profile name
        name: String,
        /// Connection protocol (optional when deriving from --profile)
        #[arg(value_parser = TransporterValueParser)]
        transporter: Option<String>,
        /// Address slot (required by most transporters)
        target: Option<String>,
        /// Content slot (optional, per transporter)
        app: Option<String>,
        /// Override transporter (--transporter wins over the positional)
        #[arg(long = "transporter", value_name = "TYPE", value_parser = TransporterValueParser)]
        transporter_flag: Option<String>,
        /// Override address slot (--target wins over the positional)
        #[arg(long = "target", value_name = "ADDR")]
        target_flag: Option<String>,
        /// Override content slot (--app wins over the positional)
        #[arg(long = "app", value_name = "IDENTIFIER")]
        app_flag: Option<String>,
        /// Backend param KEY=VALUE; repeatable
        #[arg(long = "param", value_name = "KEY=VALUE")]
        extra_params: Vec<String>,
        /// Raw args stored verbatim (`-- --no-audio -x`)
        #[arg(last = true, value_name = "RAW_ARGS")]
        raw_args: Vec<String>,
        /// Ignore BASE's raw_args (CLI passthrough becomes the whole list)
        #[arg(long)]
        clear_raw: bool,
        /// Derive from this existing profile before applying overrides
        #[arg(long = "profile", value_name = "BASE")]
        base_profile: Option<String>,
    },
    /// List saved profiles.
    List {
        /// Emit full profiles as pretty JSON instead of bare names
        #[arg(long)]
        json: bool,
    },
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

/// Print `error: …` and, for syntax-signal errors (unknown transporter,
/// missing addressing slots), follow with the full clap-derived help menu
/// so users can self-correct without re-running with `--help`.
///
/// Help text is rendered by clap from the argument definitions — it can
/// never drift from the real surface. Everything goes to stderr.
pub fn report_error(err: &anyhow::Error) {
    eprintln!("error: {err:#}");
    if !syntax_error(err) {
        return;
    }
    eprintln!();
    let mut stderr = std::io::stderr();
    let _ = Cli::command().write_long_help(&mut stderr);
    eprintln!();
}

/// Does this error indicate a usage/syntax problem (as opposed to a
/// runtime failure like an unreachable device)?
fn syntax_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause.downcast_ref::<AppError>().is_some_and(|e| {
            matches!(e, AppError::UnknownTransporter { .. } | AppError::Usage(_))
        })
    })
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
        Command::Transporters => cmd_transporters(),
    }
}

async fn cmd_run(args: RunArgs) -> anyhow::Result<()> {
    let profile = load_optional_profile(args.profile.as_deref())?;
    let config = merge_config(&args, profile)?;

    let transporter = transporters::build_registry().get(&config.transporter)?;
    debug!(transporter = transporter.name(), "dispatching to backend");
    transporter.run(&config).await?;
    Ok(())
}

fn cmd_transporters() -> anyhow::Result<()> {
    let registry = transporters::build_registry();
    for (name, origin) in registry.entries() {
        println!("{name:<14} {origin}");
    }
    Ok(())
}

async fn cmd_snapshot(args: RunArgs) -> anyhow::Result<()> {
    let profile = load_optional_profile(args.profile.as_deref())?;
    let config = merge_config(&args, profile)?;
    emit(&render_snapshot(&config))?;
    Ok(())
}

async fn cmd_list(args: ListArgs) -> anyhow::Result<()> {
    let profile = load_optional_profile(args.profile.as_deref())?;

    // Listing is backend-specific (pm list vs .desktop scan vs ...), so the
    // transporter is required here too — same explicitness as `run`. Slot
    // resolution mirrors run exactly: positional > flag > profile.
    const USAGE: &str = "appcast list <TRANSPORTER> <TARGET>";
    let transporter_name = args
        .positional_transporter
        .clone()
        .or(args.transporter)
        .or_else(|| profile.as_ref().map(|p| p.transporter.clone()))
        .ok_or(AppError::Usage(USAGE.into()))?;
    let target = args
        .positional_target
        .clone()
        .or(args.target)
        .or_else(|| profile.as_ref().and_then(|p| p.target.clone()))
        .ok_or(AppError::Usage(USAGE.into()))?;
    let params = profile.map(|p| p.params).unwrap_or_default();

    let transporter = transporters::build_registry().get(&transporter_name)?;
    let entries = transporter.list_apps(&target, &params).await?;

    // Rendering is CLI-local; the data itself is the reusable core API
    // (Serialize on AppEntry lets a future WebUI return it verbatim).
    let rendered = if args.json {
        serde_json::to_string_pretty(&entries)
            .map_err(|e| AppError::BackendError(format!("serialize entries: {e}")))?
    } else if args.long {
        render_long(&entries)
    } else {
        render_ids(&entries)
    };
    emit(&rendered)?;
    Ok(())
}

/// Write to stdout, treating a closed downstream (`appcast list … | head`)
/// as a clean exit instead of panicking on SIGPIPE-induced write errors.
fn emit(text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout();
    match out.write_all(text.as_bytes()).and_then(|_| out.flush()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Default rendering: one bare id per line (script-friendly).
fn render_ids(entries: &[AppEntry]) -> String {
    let mut out = String::new();
    for entry in entries {
        out.push_str(&entry.id);
        out.push('\n');
    }
    out
}

/// Long rendering: display name + id, tab-separated (name may be absent).
fn render_long(entries: &[AppEntry]) -> String {
    let mut out = String::new();
    for entry in entries {
        match &entry.name {
            Some(name) => out.push_str(name),
            None => out.push('-'),
        }
        out.push('\t');
        out.push_str(&entry.id);
        out.push('\n');
    }
    out
}

async fn cmd_profile(action: ProfileAction) -> anyhow::Result<()> {
    match action {
        ProfileAction::Save {
            name,
            transporter,
            target,
            app,
            transporter_flag,
            target_flag,
            app_flag,
            extra_params,
            raw_args,
            clear_raw,
            base_profile,
        } => {
            // Load the derive base first (if any), then reuse run's merge
            // machinery: a saved profile is just a frozen ResolvedConfig.
            // Flags win over positionals so slot-shift typos stay impossible.
            let transporter = transporter.or(transporter_flag);
            let target = target.or(target_flag);
            let app = app.or(app_flag);
            let base = load_optional_profile(base_profile.as_deref())?;
            let run_like = RunArgs {
                positional_transporter: transporter,
                positional_target: target,
                positional_app: app,
                extra_params,
                raw_args,
                clear_raw,
                ..RunArgs::default()
            };
            const USAGE: &str =
                "appcast profile save <NAME> [TRANSPORTER] [TARGET] [APP] [--profile BASE]";
            let resolved = merge_config(&run_like, base).map_err(|e| match e {
                AppError::MissingTransporter => AppError::Usage(USAGE.into()),
                other => other,
            })?;

            // Defense-in-depth: base profiles may carry a stale name that
            // never passed the clap parser.
            transporters::build_registry()
                .get(&resolved.transporter)
                .map(|_| ())?;

            let new_profile = Profile::from(resolved);
            let path = profile::save_profile(&name, &new_profile)?;
            println!("saved profile `{name}` → {}", path.display());
            println!("{}", render_save_summary(&new_profile));
        }
        ProfileAction::List { json } => {
            let names = profile::list_profiles()?;
            if !json {
                for name in &names {
                    println!("{name}");
                }
                return Ok(());
            }
            // JSON shape: [{ name, <flattened profile fields> }, ...]
            #[derive(serde::Serialize)]
            struct ProfileEntry<'a> {
                name: &'a str,
                #[serde(flatten)]
                profile: &'a Profile,
            }
            let mut loaded = Vec::new();
            for name in &names {
                // One corrupt file must not take down the whole listing.
                match profile::load_profile(name) {
                    Ok(p) => loaded.push((name.clone(), p)),
                    Err(e) => eprintln!("warning: skipping profile `{name}`: {e}"),
                }
            }
            let entries: Vec<ProfileEntry> = loaded
                .iter()
                .map(|(name, profile)| ProfileEntry { name, profile })
                .collect();
            let rendered = serde_json::to_string_pretty(&entries)
                .map_err(|e| AppError::BackendError(format!("serialize profiles: {e}")))?;
            emit(&rendered)?;
        }
        ProfileAction::Edit { name } => profile::edit_profile(&name).await?,
        ProfileAction::Rm { name } => {
            profile::delete_profile(&name)?;
            println!("removed profile `{name}`");
        }
    }
    Ok(())
}

/// Echo the parsed slots after saving so positional misalignment is
/// immediately visible (`-` marks an absent slot).
fn render_save_summary(profile: &Profile) -> String {
    fn dash(value: &Option<String>) -> &str {
        value.as_deref().unwrap_or("-")
    }
    format!(
        "  transporter={} target={} app={}\n  params{{{}}} raw[{}]",
        profile.transporter,
        dash(&profile.target),
        dash(&profile.app),
        profile
            .params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" "),
        profile.raw_args.join(" "),
    )
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

    // ---- raw passthrough: profile args first, then CLI additions appended
    //      in order (scrcpy-style last-wins makes tail overrides possible);
    //      --clear-raw discards the profile base entirely ----
    let mut raw_args = Vec::new();
    if !args.clear_raw {
        raw_args.extend(p_raw_args);
    }
    raw_args.extend(args.raw_args.iter().cloned());

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
    fn raw_args_append_and_clear_semantics() {
        // Bare run → profile's raw_args flow through.
        let args = positional("adb", "d", "p");
        let config = merge_config(&args, Some(sample_profile())).unwrap();
        assert_eq!(config.raw_args, vec!["--keep-active"]);

        // CLI additions append AFTER the profile base (last-wins friendly).
        let mut append_args = positional("adb", "d", "p");
        append_args.raw_args = vec!["-x".into()];
        let config = merge_config(&append_args, Some(sample_profile())).unwrap();
        assert_eq!(config.raw_args, vec!["--keep-active", "-x"]);

        // --clear-raw drops the profile base; additions may be empty.
        let mut clear_args = positional("adb", "d", "p");
        clear_args.clear_raw = true;
        let config = merge_config(&clear_args, Some(sample_profile())).unwrap();
        assert!(config.raw_args.is_empty());

        // Clear + fresh content.
        let mut rebuild_args = positional("adb", "d", "p");
        rebuild_args.clear_raw = true;
        rebuild_args.raw_args = vec!["--no-audio".into()];
        let config = merge_config(&rebuild_args, Some(sample_profile())).unwrap();
        assert_eq!(config.raw_args, vec!["--no-audio"]);
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
    fn list_renderers_cover_all_shapes() {
        use crate::core::transporter::AppEntry;
        let entries = vec![
            AppEntry {
                id: "com.a.b".into(),
                name: Some("名字 B".into()),
                meta: Default::default(),
            },
            AppEntry::id_only("bare.id"),
        ];

        assert_eq!(render_ids(&entries), "com.a.b\nbare.id\n");
        assert_eq!(render_long(&entries), "名字 B\tcom.a.b\n-\tbare.id\n");

        let json = serde_json::to_string(&entries).unwrap();
        assert!(json.contains(r#""name":"名字 B""#));
        assert!(json.contains(r#""id":"bare.id""#));
    }

    #[test]
    fn profile_list_json_shape_is_flattened() {
        let p = sample_profile();
        #[derive(serde::Serialize)]
        struct ProfileEntry<'a> {
            name: &'a str,
            #[serde(flatten)]
            profile: &'a Profile,
        }
        let json = serde_json::to_string(&ProfileEntry {
            name: "qq",
            profile: &p,
        })
        .unwrap();
        assert!(json.contains(r#""name":"qq""#));
        assert!(json.contains(r#""transporter":"ssh-x11""#));
        assert!(json.contains(r#""raw_args":["--keep-active"]"#));
    }

    #[test]
    fn syntax_errors_trigger_help_menu() {
        let usage: anyhow::Error = AppError::Usage("run adb-scrcpy <t> <a>".into()).into();
        let unknown: anyhow::Error = AppError::UnknownTransporter {
            name: "adb".into(),
            available: "adb-scrcpy".into(),
        }
        .into();
        assert!(syntax_error(&usage));
        assert!(syntax_error(&unknown));
        // runtime failures must not dump the menu
        let runtime: anyhow::Error = AppError::DeviceNotFound("dev".into()).into();
        assert!(!syntax_error(&runtime));
    }

    #[test]
    fn save_summary_reflects_slots_and_extensions() {
        use crate::config::profile::Profile;
        let p = Profile {
            transporter: "adb-scrcpy".into(),
            target: Some("localhost:45555".into()),
            app: Some("bin.mt.plus.canary".into()),
            params: [("fps".to_string(), "90".to_string())].into_iter().collect(),
            raw_args: vec!["--no-audio".into()],
        };
        assert_eq!(
            render_save_summary(&p),
            "  transporter=adb-scrcpy target=localhost:45555 app=bin.mt.plus.canary\n  \
             params{fps=90} raw[--no-audio]"
        );
        // Absent slots render as dashes instead of empty strings.
        let minimal = Profile {
            transporter: "web".into(),
            target: Some("https://x".into()),
            app: None,
            params: Default::default(),
            raw_args: vec![],
        };
        assert!(render_save_summary(&minimal).contains("app=-"));
    }

    #[test]
    fn transporter_parser_rejects_unknown_and_suggests() {
        use std::ffi::OsStr;
        let cmd = Cli::command();

        let ok = TransporterValueParser
            .parse_ref(&cmd, None, OsStr::new("adb-scrcpy"))
            .unwrap();
        assert_eq!(ok, "adb-scrcpy");

        // typo close to a real name -> suggestion attached
        let err = TransporterValueParser
            .parse_ref(&cmd, None, OsStr::new("adb-scrcp"))
            .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
        let rendered = err.render().to_string();
        assert!(rendered.contains("adb-scrcpy"), "{rendered}");
        assert!(rendered.contains("tip: a similar value exists"), "{rendered}");

        // prefix relationships also count as close ('adb' -> 'adb-scrcpy')
        let err = TransporterValueParser
            .parse_ref(&cmd, None, OsStr::new("adb"))
            .unwrap_err();
        assert!(err.render().to_string().contains("tip: a similar value exists"));
    }

    #[test]
    fn shell_quotes_unsafe_values() {
        assert_eq!(shell_quote("plain-value"), "plain-value");
        assert_eq!(shell_quote("with space"), "'with space'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
