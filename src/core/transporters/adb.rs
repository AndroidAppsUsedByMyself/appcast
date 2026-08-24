//! `AndroidAdbTransporter` — cast an Android app into a local window.
//!
//! Single, battle-tested pipeline: one scrcpy (≥ 3) process creates the
//! virtual display at the configured resolution and starts the app itself,
//! mirroring it immediately. Killing scrcpy (window close / Ctrl+C / app
//! exit) destroys the virtual display — no manual display bookkeeping.
//!
//! Equivalent to:
//! ```text
//! scrcpy -s <target> --new-display=<WxH> --start-app=<package>
//!        [--no-vd-destroy-content] --max-fps <fps> --video-bit-rate <n>M
//! ```
//!
//! Tuning params (via `--param` / profile `params`) — this backend owns both
//! interpretation and defaults:
//! - `adb_path`, `scrcpy_path`: custom binaries
//! - `resolution`: `<W>x<H>` for the virtual display (default `1920x1080`)
//! - `fps`: frame rate cap (default `60`)
//! - `bit_rate`: video bit rate in Mbps (default `8`, sent as `<n>M`)
//!
//! Everything else scrcpy offers goes through raw args after `--`, appended
//! verbatim to the command line (e.g. `-- --no-vd-destroy-content
//! --video-codec=h265 -x`). Later duplicates win per scrcpy's own parser, so
//! explicit overrides are possible.
//!
//! Apps are started whole-package via `--start-app`; intent-level launching
//! (`-a/-d/-t/--es ...`) would require an `am start` pipeline — a separate
//! transporter for forks to build on top of the same trait.

use std::collections::HashMap;
use std::env::consts::EXE_SUFFIX;
use std::process::Stdio;

use tokio::process::{Child, Command};
use tracing::{debug, info, warn};

use crate::core::error::AppError;
use crate::core::transporter::{BoxFut, ResolvedConfig, Transporter};

/// The adb + scrcpy virtual-display backend (the only implemented backend).
pub struct AndroidAdbTransporter;

/// Param keys this backend actually interprets; anything else triggers a
/// warning (typos fail loudly, forks can extend the set in their own backend).
const KNOWN_PARAMS: &[&str] = &["adb_path", "scrcpy_path", "resolution", "fps", "bit_rate"];

/// Backend-owned defaults, applied whenever the matching param is absent.
const DEFAULT_RESOLUTION: (u32, u32) = (1920, 1080);
const DEFAULT_FPS: u32 = 60;
const DEFAULT_BIT_RATE_MBPS: u32 = 8;

impl AndroidAdbTransporter {
    /// Collect param keys this backend does not interpret.
    fn unknown_param_keys(
        params: &std::collections::HashMap<String, String>,
    ) -> Vec<&str> {
        params
            .keys()
            .filter(|k| !KNOWN_PARAMS.contains(&k.as_str()))
            .map(String::as_str)
            .collect()
    }

    /// Virtual display size: `resolution` param or the built-in default.
    fn resolution(config: &ResolvedConfig) -> Result<(u32, u32), AppError> {
        match config.param("resolution") {
            None => Ok(DEFAULT_RESOLUTION),
            Some(value) => parse_wxh(value),
        }
    }

    /// Frame rate: `fps` param or the built-in default.
    fn fps(config: &ResolvedConfig) -> Result<u32, AppError> {
        match config.param("fps") {
            None => Ok(DEFAULT_FPS),
            Some(value) => value
                .trim()
                .parse()
                .map_err(|_| AppError::InvalidParamValue {
                    key: "fps".into(),
                    value: value.into(),
                }),
        }
    }

    /// Bit rate in Mbps: `bit_rate` param or the built-in default.
    fn bit_rate(config: &ResolvedConfig) -> Result<u32, AppError> {
        match config.param("bit_rate") {
            None => Ok(DEFAULT_BIT_RATE_MBPS),
            Some(value) => value
                .trim()
                .parse()
                .map_err(|_| AppError::InvalidParamValue {
                    key: "bit_rate".into(),
                    value: value.into(),
                }),
        }
    }

    /// Resolve the adb binary: `params["adb_path"]` wins, else plain `adb`
    /// from PATH. `EXE_SUFFIX` keeps this Windows-compatible (`adb.exe`).
    fn adb_bin(config: &ResolvedConfig) -> String {
        config
            .param("adb_path")
            .map(str::to_owned)
            .unwrap_or_else(|| format!("adb{EXE_SUFFIX}"))
    }

    /// Resolve the scrcpy binary the same way (`params["scrcpy_path"]`).
    fn scrcpy_bin(config: &ResolvedConfig) -> String {
        config
            .param("scrcpy_path")
            .map(str::to_owned)
            .unwrap_or_else(|| format!("scrcpy{EXE_SUFFIX}"))
    }

    /// Base command shaped `adb -s <target> ...`.
    fn adb_cmd(bin: &str, target: &str) -> Command {
        let mut cmd = Command::new(bin);
        cmd.arg("-s").arg(target);
        cmd
    }

    /// Run a command and return captured stdout; non-zero exit or spawn
    /// failure becomes [`AppError::BackendError`] with stderr appended.
    async fn capture(mut cmd: Command, what: &str) -> Result<String, AppError> {
        let out = cmd
            .output()
            .await
            .map_err(|e| AppError::BackendError(format!("{what}: {e}")))?;
        if !out.status.success() {
            return Err(AppError::BackendError(format!(
                "{what}: exit {status}\n{}",
                String::from_utf8_lossy(&out.stderr).trim(),
                status = out.status
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Device reachability probe: `adb -s <t> shell echo ok` must echo `ok`.
    async fn check_device(&self, config: &ResolvedConfig) -> Result<(), AppError> {
        let bin = Self::adb_bin(config);
        let mut cmd = Self::adb_cmd(&bin, &config.target);
        cmd.args(["shell", "echo", "ok"]);
        let out = cmd
            .output()
            .await
            .map_err(|e| AppError::DeviceNotFound(format!("cannot execute `{bin}`: {e}")))?;
        if out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "ok" {
            Ok(())
        } else {
            Err(AppError::DeviceNotFound(config.target.clone()))
        }
    }

    /// Sanity-check the package name: reject emptiness, path separators
    /// (`/`, `\`) and traversal-ish input so a Linux path like
    /// `/usr/bin/firefox` fails fast with a precise error.
    fn validate_package_name(app: &str) -> Result<(), AppError> {
        let looks_like_path = app.contains('/') || app.contains('\\') || app.contains("..");
        if app.is_empty() || looks_like_path || app.split_whitespace().count() != 1 {
            return Err(AppError::InvalidAppIdentifier(app.to_string()));
        }
        Ok(())
    }

    /// Build the scrcpy argument list (pure, unit-testable): fixed pipeline
    /// first, then the user's raw passthrough args verbatim at the tail.
    fn scrcpy_args(config: &ResolvedConfig) -> Result<Vec<String>, AppError> {
        let (width, height) = Self::resolution(config)?;
        let fps = Self::fps(config)?;
        let bit_rate = Self::bit_rate(config)?;

        let mut args = vec![
            "-s".to_string(),
            config.target.clone(),
            format!("--new-display={width}x{height}"),
            format!("--start-app={}", config.app),
        ];
        args.push("--max-fps".to_string());
        args.push(fps.to_string());
        args.push("--video-bit-rate".to_string());
        args.push(format!("{bit_rate}M"));
        // Passthrough: every remaining scrcpy option, appended verbatim.
        args.extend(config.raw_args.iter().cloned());
        Ok(args)
    }

    /// Spawn scrcpy with the full virtual-display pipeline.
    async fn spawn_scrcpy(&self, config: &ResolvedConfig) -> Result<Child, AppError> {
        let bin = Self::scrcpy_bin(config);

        let mut cmd = Command::new(&bin);
        cmd.args(Self::scrcpy_args(config)?);
        // Silence scrcpy's chatty stdout; stderr stays inherited so real
        // errors remain visible.
        cmd.stdout(Stdio::null());
        // Safety net: kill scrcpy even if the future is dropped unexpectedly.
        cmd.kill_on_drop(true);

        cmd.spawn()
            .map_err(|e| AppError::ScrcpySpawnFailed(format!("{bin}: {e}")))
    }

    /// Block until scrcpy exits naturally or the user hits Ctrl+C.
    async fn wait_for_exit(child: &mut Child) -> String {
        tokio::select! {
            status = child.wait() => match status {
                Ok(status) => format!("scrcpy exited ({status})"),
                Err(e) => format!("scrcpy crashed: {e}"),
            },
            _ = tokio::signal::ctrl_c() => "interrupted by Ctrl+C".to_string(),
        }
    }

    /// Mandatory teardown regardless of exit path: kill any lingering
    /// scrcpy process and reap it (the virtual display dies with it).
    async fn reap_child(child: &mut Child) {
        if child.id().is_some() {
            debug!("killing lingering scrcpy process");
            let _ = child.kill().await;
        }
        let _ = child.wait().await; // reap, ignore errors
    }
}

impl Transporter for AndroidAdbTransporter {
    fn name(&self) -> &'static str {
        "adb"
    }

    fn run<'a>(&'a self, config: &'a ResolvedConfig) -> BoxFut<'a, Result<(), AppError>> {
        Box::pin(async move {
            // Fail fast on obviously-wrong identifiers before any subprocess.
            Self::validate_package_name(&config.app)?;

            // Surface likely typos in --param keys (they would be inert).
            for key in Self::unknown_param_keys(&config.params) {
                warn!(key, "unknown param for adb backend (known: adb_path, scrcpy_path, resolution, fps, bit_rate); ignoring");
            }

            self.check_device(config).await?;

            let mut child = self.spawn_scrcpy(config).await?;
            info!(
                target = %config.target,
                app = %config.app,
                "scrcpy virtual display started; press Ctrl+C to stop"
            );

            let exit_reason = Self::wait_for_exit(&mut child).await;
            Self::reap_child(&mut child).await;

            info!(reason = %exit_reason, "session finished");
            Ok(())
        })
    }

    fn list_apps<'a>(
        &'a self,
        target: &'a str,
        params: &'a HashMap<String, String>,
    ) -> BoxFut<'a, Result<Vec<String>, AppError>> {
        Box::pin(async move {
            let adb_path = params
                .get("adb_path")
                .cloned()
                .unwrap_or_else(|| format!("adb{EXE_SUFFIX}"));
            let mut cmd = Self::adb_cmd(&adb_path, target);
            cmd.args(["shell", "pm", "list", "packages"]);
            let stdout = Self::capture(cmd, "pm list packages").await?;

            let mut apps: Vec<String> = stdout
                .lines()
                .filter_map(|line| line.strip_prefix("package:"))
                .map(str::to_owned)
                .collect();
            apps.sort();
            Ok(apps)
        })
    }
}

/// Parse a `"WxH"` string into its `(width, height)` components.
///
/// # Errors
/// [`AppError::InvalidResolutionFormat`] when the input is not `<u32>x<u32>`.
fn parse_wxh(value: &str) -> Result<(u32, u32), AppError> {
    let invalid = || AppError::InvalidResolutionFormat(value.to_string());
    let (w, h) = value.split_once('x').ok_or_else(invalid)?;
    let w: u32 = w.trim().parse().map_err(|_| invalid())?;
    let h: u32 = h.trim().parse().map_err(|_| invalid())?;
    Ok((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(raw_args: &[&str], params: &[(&str, &str)]) -> ResolvedConfig {
        ResolvedConfig {
            transporter: "adb".into(),
            target: "10.0.0.8:5555".into(),
            app: "com.termux".into(),
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            raw_args: raw_args.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn defaults_apply_when_params_absent() {
        let cfg = config(&[], &[]);
        assert_eq!(AndroidAdbTransporter::resolution(&cfg).unwrap(), (1920, 1080));
        assert_eq!(AndroidAdbTransporter::fps(&cfg).unwrap(), 60);
        assert_eq!(AndroidAdbTransporter::bit_rate(&cfg).unwrap(), 8);
    }

    #[test]
    fn param_values_override_defaults_or_fail_loudly() {
        let cfg = config(
            &[],
            &[("resolution", "1280x960"), ("fps", "90"), ("bit_rate", "12")],
        );
        assert_eq!(AndroidAdbTransporter::resolution(&cfg).unwrap(), (1280, 960));
        assert_eq!(AndroidAdbTransporter::fps(&cfg).unwrap(), 90);
        assert_eq!(AndroidAdbTransporter::bit_rate(&cfg).unwrap(), 12);

        let bad = config(&[], &[("resolution", "big")]);
        assert!(matches!(
            AndroidAdbTransporter::resolution(&bad),
            Err(AppError::InvalidResolutionFormat(_))
        ));
        let bad_fps = config(&[], &[("fps", "fast")]);
        assert!(matches!(
            AndroidAdbTransporter::fps(&bad_fps),
            Err(AppError::InvalidParamValue { key, .. }) if key == "fps"
        ));
    }

    #[test]
    fn builds_scrcpy_pipeline_and_appends_passthrough() {
        let cfg = config(
            &["--no-vd-destroy-content", "--video-codec=h265", "-x"],
            &[],
        );
        assert_eq!(
            AndroidAdbTransporter::scrcpy_args(&cfg).unwrap(),
            vec![
                "-s",
                "10.0.0.8:5555",
                "--new-display=1920x1080",
                "--start-app=com.termux",
                "--max-fps",
                "60",
                "--video-bit-rate",
                "8M",
                // passthrough lands verbatim at the tail
                "--no-vd-destroy-content",
                "--video-codec=h265",
                "-x",
            ]
        );
    }

    #[test]
    fn minimal_config_has_no_extra_flags() {
        let cfg = config(&[], &[]);
        let args = AndroidAdbTransporter::scrcpy_args(&cfg).unwrap();
        assert_eq!(args.last().map(String::as_str), Some("8M"));
        assert!(!args.contains(&"--no-vd-destroy-content".to_string()));
    }

    #[test]
    fn flags_unknown_params_only() {
        let cfg = config(&[], &[("adb_path", "/x"), ("scrcpy_pathh", "typo")]);
        assert_eq!(
            AndroidAdbTransporter::unknown_param_keys(&cfg.params),
            vec!["scrcpy_pathh"]
        );
        assert!(AndroidAdbTransporter::unknown_param_keys(
            &config(
                &[],
                &[
                    ("adb_path", "/x"),
                    ("scrcpy_path", "/y"),
                    ("resolution", "1x1"),
                    ("fps", "30"),
                    ("bit_rate", "4"),
                ]
            )
            .params
        )
        .is_empty());
    }

    #[test]
    fn rejects_pathlike_identifiers() {
        assert!(matches!(
            AndroidAdbTransporter::validate_package_name("/usr/bin/firefox"),
            Err(AppError::InvalidAppIdentifier(_))
        ));
        assert!(matches!(
            AndroidAdbTransporter::validate_package_name("com/a"),
            Err(AppError::InvalidAppIdentifier(_))
        ));
        assert!(matches!(
            AndroidAdbTransporter::validate_package_name(""),
            Err(AppError::InvalidAppIdentifier(_))
        ));
        assert!(matches!(
            AndroidAdbTransporter::validate_package_name("two words"),
            Err(AppError::InvalidAppIdentifier(_))
        ));
        assert!(
            AndroidAdbTransporter::validate_package_name("com.tencent.mm").is_ok(),
            "ordinary package names must pass"
        );
    }
}
