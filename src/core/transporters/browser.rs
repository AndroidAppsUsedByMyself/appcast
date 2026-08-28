//! `WebBrowserTransporter` — cast a web app into a desktop window.
//!
//! Zero-dependency backend: it shells out to whatever browser the machine
//! already has, mirroring how adb-scrcpy shells out to scrcpy. Two launch
//! strategies, chosen by engine family:
//!
//! - Chromium-family (default probe chain, or any unknown binary):
//!   `--app=<url>` opens a standalone window without tabs/address bar —
//!   the closest thing to a native webview that needs no extra install.
//! - Gecko-family (detected by name: firefox, librewolf): no app mode
//!   exists, so only fullscreen `-kiosk <url>` is offered; refusing plain
//!   tab windows keeps "casting" semantics honest.
//!
//! Addressing schema: `target` = http(s) URL; `app` is unused (the URL is
//! the content). `list_apps` returns an empty list — a web target has no
//! enumerable apps by nature, which is semantic completeness rather than a
//! missing feature.
//!
//! Params (backend-owned interpretation and defaults):
//! - `browser_path`: explicit browser binary; wins over auto-detection.
//!   Unknown binary names are assumed Chromium-compatible.
//! - `window_size`: `<WxH>`, forwarded as `--window-size=W,H`
//!   (Chromium only — Gecko kiosk is always fullscreen).
//! - `kiosk`: `true`/`false` (default false); Chromium `--kiosk`,
//!   mandatory for Gecko.
//! - `profile`: browser profile isolation. Default = a dedicated
//!   appcast-owned directory (persistent logins across casts, independent
//!   of the daily browser). `default` = the browser's own profile
//!   (shares cookies; subject to single-instance handoff, see below);
//!   any other value = used verbatim as a custom profile path.
//!
//! Chromium-family browsers are single-instance per profile: a launch
//! against an already-running profile hands the URL to that process and
//! exits immediately. The dedicated default profile avoids stealing the
//! user's running session AND keeps the spawned process under our
//! lifecycle control; `--new-window` makes even a delegated relaunch open
//! its own window instead of a tab.
//!
//! Everything else goes through raw args after `--`, appended verbatim.

use std::collections::HashMap;
use std::env::consts::EXE_SUFFIX;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::process::Stdio;

use tokio::process::{Child, Command};
use tracing::{debug, info, warn};

use crate::core::error::AppError;
use crate::core::transporter::{AppEntry, BoxFut, ResolvedConfig, Transporter};
use crate::core::transporters::session;
use crate::utils::parse::parse_wxh;

/// The shell-out browser backend (`--app` mode / gecko kiosk).
pub struct WebBrowserTransporter;

/// Param keys this backend actually interprets; anything else triggers a
/// warning so typos fail loudly instead of staying inert.
const KNOWN_PARAMS: &[&str] = &["browser_path", "window_size", "kiosk", "profile"];

/// Browser engine families this backend knows how to drive. Detection is
/// purely name-based; anything unrecognised is assumed Chromium-compatible
/// (the dominant CLI convention) — forks extend [`family_of`] for others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// Standalone-window mode: `--app=<url> [--window-size=W,H] [--kiosk]`.
    Chromium,
    /// No app mode exists: fullscreen `-kiosk <url>` or nothing.
    Gecko,
}

impl WebBrowserTransporter {
    /// The addressing schema of this backend: exactly one slot, and it must
    /// be a real http(s) URL — typos like bare hostnames fail fast here.
    fn validate_url(url: &str) -> Result<(), AppError> {
        let lower = url.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            Ok(())
        } else {
            Err(AppError::InvalidUrl(url.to_string()))
        }
    }

    /// Engine family from the binary's file name (not its directory, which
    /// often carries vendor names like "Mozilla Firefox").
    fn family_of(bin: &str) -> Family {
        let base = Path::new(bin)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(bin)
            .to_ascii_lowercase();
        if base.contains("firefox") || base.contains("librewolf") {
            Family::Gecko
        } else {
            Family::Chromium
        }
    }

    /// Truthy/falsy param parsing for `kiosk`; anything else is a typo and
    /// must fail loudly rather than silently mean false.
    fn parse_kiosk(config: &ResolvedConfig) -> Result<bool, AppError> {
        match config.param("kiosk") {
            None => Ok(false),
            Some(value) => match value.trim() {
                "true" | "1" | "yes" => Ok(true),
                "false" | "0" | "no" | "" => Ok(false),
                other => Err(AppError::InvalidParamValue {
                    key: "kiosk".into(),
                    value: other.into(),
                }),
            },
        }
    }

    /// Dedicated persistent profile dir under the appcast data root:
    /// `$XDG_DATA_HOME/appcast/web-browser-profile` on Linux,
    /// `%APPDATA%\appcast\web-browser-profile` on Windows. Logins made in
    /// a cast survive across runs without touching the daily browser.
    fn default_profile() -> std::path::PathBuf {
        use etcetera::app_strategy::{choose_app_strategy, AppStrategy, AppStrategyArgs};
        let args = || AppStrategyArgs {
            top_level_domain: "io.github".into(),
            author: "AndroidAppsUsedByMyself".into(),
            app_name: "appcast".into(),
        };
        choose_app_strategy(args())
            .map(|s| s.data_dir().join("web-browser-profile"))
            .unwrap_or_else(|_| std::env::temp_dir().join("appcast-web-browser-profile"))
    }

    /// Collect param keys this backend does not interpret.
    fn unknown_param_keys(params: &HashMap<String, String>) -> Vec<&str> {
        params
            .keys()
            .filter(|k| !KNOWN_PARAMS.contains(&k.as_str()))
            .map(String::as_str)
            .collect()
    }

    /// Resolve the browser binary: explicit `browser_path` first (trusted
    /// verbatim — spawn errors surface naturally), else walk the platform
    /// probe chain. Only Chromium-family browsers are auto-detected: they
    /// are the only ones with a usable windowed mode.
    fn resolve_browser(config: &ResolvedConfig) -> Result<String, AppError> {
        if let Some(explicit) = config.param("browser_path") {
            return Ok(explicit.to_owned());
        }
        let candidates = platform_candidates();
        for candidate in &candidates {
            if resolves(candidate) {
                debug!(browser = %candidate, "auto-detected browser");
                return Ok(candidate.clone());
            }
        }
        Err(AppError::NoBrowserFound {
            tried: candidates.join(", "),
        })
    }

    /// Build the argument list (pure, unit-testable): family-specific
    /// pipeline first, then the user's raw passthrough args at the tail.
    fn launch_plan(
        bin: &str,
        url: &str,
        config: &ResolvedConfig,
    ) -> Result<Vec<String>, AppError> {
        let window = match config.param("window_size") {
            None => None,
            Some(value) => Some(parse_wxh(value)?),
        };
        let kiosk = Self::parse_kiosk(config)?;

        let mut args = Vec::new();
        match Self::family_of(bin) {
            Family::Chromium => {
                args.push(format!("--app={url}"));
                if let Some((w, h)) = window {
                    args.push(format!("--window-size={w},{h}"));
                }
                if kiosk {
                    args.push("--kiosk".to_string());
                }
                match config.param("profile") {
                    Some("default") => {} // share the browser's own profile
                    Some(custom) => args.push(format!("--user-data-dir={custom}")),
                    None => args.push(format!("--user-data-dir={}", Self::default_profile().display())),
                    // Even against a running instance this opens its own
                    // window instead of delegating into a tab.
                }
                args.push("--new-window".to_string());
            }
            Family::Gecko => {
                // A plain `firefox <url>` just opens another tab in a normal
                // browsing session — not a cast. Kiosk is the only mode with
                // dedicated-window semantics, so require it explicitly.
                if !kiosk {
                    return Err(AppError::BackendError(format!(
                        "`{bin}` looks like a Firefox-family browser (no app mode); \
                         add --param kiosk=true for fullscreen kiosk casting"
                    )));
                }
                if window.is_some() {
                    warn!("param `window_size` has no effect on Firefox-family kiosk; ignoring");
                }
                args.push("-kiosk".to_string());
                args.push(url.to_string());
                // Refuse delegation to an already-running firefox: own
                // process or nothing, so Ctrl+C semantics stay honest.
                args.push("--new-instance".to_string());
            }
        }
        args.extend(config.raw_args.iter().cloned());
        Ok(args)
    }

    /// Spawn the browser with stdout/stderr silenced: browsers log GPU and
    /// policy noise on both streams while reporting real failures through
    /// their own UI. Lifecycle mirrors the adb backend.
    async fn spawn_browser(&self, bin: &str, url: &str, config: &ResolvedConfig)
        -> Result<Child, AppError>
    {
        let mut cmd = Command::new(bin);
        cmd.args(Self::launch_plan(bin, url, config)?);
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        // Safety net: close the window even if the future is dropped.
        cmd.kill_on_drop(true);

        cmd.spawn()
            .map_err(|e| AppError::BrowserLaunchFailed(format!("{bin}: {e}")))
    }
}

impl Transporter for WebBrowserTransporter {
    fn name(&self) -> &'static str {
        "web-browser"
    }

    fn description(&self) -> &'static str {
        "System browser in app mode — Chromium --app or Firefox kiosk"
    }

    fn run<'a>(&'a self, config: &'a ResolvedConfig) -> BoxFut<'a, Result<(), AppError>> {
        Box::pin(async move {
            const USAGE: &str = "appcast run web-browser <https-url>";
            let url = config
                .target
                .as_deref()
                .ok_or_else(|| AppError::Usage(USAGE.into()))?;

            Self::validate_url(url)?;
            for key in Self::unknown_param_keys(&config.params) {
                warn!(key, "unknown param for web-browser backend (known: browser_path, window_size, kiosk); ignoring");
            }

            let bin = Self::resolve_browser(config)?;
            let mut child = self.spawn_browser(&bin, url, config).await?;
            info!(browser = %bin, url = %url, "web app window opened; press Ctrl+C to stop");

            let exit_reason = session::wait_or_ctrl_c(&mut child).await;
            session::reap(&mut child).await;

            info!(reason = %exit_reason, "session finished");
            Ok(())
        })
    }

    fn list_apps<'a>(
        &'a self,
        _target: &'a str,
        _params: &'a HashMap<String, String>,
    ) -> BoxFut<'a, Result<Vec<AppEntry>, AppError>> {
        // Semantic emptiness, not a stub: there is nothing to enumerate on
        // the web. (A fork could read browser bookmarks here.)
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// Platform default probe chain, best first. Absolute install locations
/// come before bare names because Windows browsers rarely join `$PATH`,
/// while Unix installs usually do. Deliberately Chromium-only — Gecko
/// browsers have no app mode, so picking one silently would surprise.
fn platform_candidates() -> Vec<String> {
    #[cfg(windows)]
    {
        let mut v = Vec::new();
        let mut push = |root_key: &str, sub: &[&str]| {
            if let Some(root) = std::env::var_os(root_key) {
                let mut p = PathBuf::from(root);
                for part in sub {
                    p.push(part);
                }
                v.push(p.to_string_lossy().into_owned());
            }
        };
        // Edge ships with every Windows install; Chrome covers the rest.
        for root in ["ProgramFiles(x86)", "ProgramFiles"] {
            push(root, &["Microsoft", "Edge", "Application", "msedge.exe"]);
            push(root, &["Google", "Chrome", "Application", "chrome.exe"]);
        }
        push("LocalAppData", &["Google", "Chrome", "Application", "chrome.exe"]);
        v.push(format!("msedge{EXE_SUFFIX}"));
        v.push(format!("chrome{EXE_SUFFIX}"));
        return v;
    }

    #[cfg(target_os = "macos")]
    {
        vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into(),
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".into(),
            "/Applications/Chromium.app/Contents/MacOS/Chromium".into(),
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser".into(),
        ]
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        ["chromium", "chromium-browser", "google-chrome-stable", "google-chrome",
         "brave-browser", "brave"]
            .iter()
            .map(|name| format!("{name}{EXE_SUFFIX}"))
            .collect()
    }
}

/// Whether `bin` would likely spawn: paths (absolute or containing a
/// separator) must exist as files; bare names are looked up across `$PATH`.
/// This mirrors what the OS spawn layer will do, minus PATHEXT subtleties.
fn resolves(bin: &str) -> bool {
    let direct = Path::new(bin);
    let looks_like_path =
        direct.is_absolute() || bin.contains('/') || bin.contains('\\');
    if looks_like_path {
        return direct.is_file();
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let file = dir.join(bin);
            if file.is_file() {
                return true;
            }
            #[cfg(windows)]
            // `chrome` → `chrome.exe`, matching std's own spawn behaviour.
            if let Some(stem) = file.file_stem() {
                if stem != file.file_name().unwrap_or_default()
                    && file.with_extension("exe").is_file()
                {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(raw_args: &[&str], params: &[(&str, &str)]) -> ResolvedConfig {
        ResolvedConfig {
            transporter: "web-browser".into(),
            target: Some("https://example.com".into()),
            app: None,
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            raw_args: raw_args.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn accepts_only_http_s_urls() {
        for good in ["http://a", "https://a/b?c=d", "HTTPS://UPPER.COM"] {
            assert!(WebBrowserTransporter::validate_url(good).is_ok(), "{good}");
        }
        for bad in ["", "example.com", "ftp://x", "file:///tmp/a", "https:/missing"] {
            assert!(matches!(
                WebBrowserTransporter::validate_url(bad),
                Err(AppError::InvalidUrl(_))
            ), "{bad}");
        }
    }

    #[test]
    fn family_detection_by_base_name() {
        assert_eq!(
            WebBrowserTransporter::family_of("/usr/bin/firefox"),
            Family::Gecko
        );
        assert_eq!(
            WebBrowserTransporter::family_of(r"C:\Program Files\Mozilla Firefox\firefox.exe"),
            Family::Gecko
        );
        assert_eq!(
            WebBrowserTransporter::family_of("/usr/bin/librewolf"),
            Family::Gecko
        );
        assert_eq!(
            WebBrowserTransporter::family_of("/usr/bin/chromium"),
            Family::Chromium
        );
        assert_eq!(
            WebBrowserTransporter::family_of(r"C:\x\msedge.exe"),
            Family::Chromium
        );
        // Unknown binaries default to the dominant convention.
        assert_eq!(
            WebBrowserTransporter::family_of("/usr/bin/surf"),
            Family::Chromium
        );
    }

    #[test]
    fn chromium_minimal_plan_isolates_profile_and_new_window() {
        let args =
            WebBrowserTransporter::launch_plan("/usr/bin/chromium", "https://e.com", &config(&[], &[]))
                .unwrap();
        assert_eq!(args[0], "--app=https://e.com");
        let user_data = args
            .iter()
            .find(|a| a.starts_with("--user-data-dir="))
            .expect("dedicated profile dir is injected by default");
        assert!(user_data.contains("appcast"), "{user_data}");
        assert!(args.contains(&"--new-window".to_string()));
    }

    #[test]
    fn profile_param_switches_isolation_modes() {
        // `default` = share the browser's own profile: no --user-data-dir.
        let shared = config(&[], &[("profile", "default")]);
        let args =
            WebBrowserTransporter::launch_plan("/usr/bin/chromium", "https://e.com", &shared)
                .unwrap();
        assert!(!args.iter().any(|a| a.starts_with("--user-data-dir=")));
        assert!(args.contains(&"--new-window".to_string()));

        // Any other value = custom profile path, verbatim.
        let custom = config(&[], &[("profile", "/tmp/cast-profile")]);
        let args =
            WebBrowserTransporter::launch_plan("/usr/bin/chromium", "https://e.com", &custom)
                .unwrap();
        assert!(args.contains(&"--user-data-dir=/tmp/cast-profile".to_string()));
    }

    #[test]
    fn chromium_full_plan_orders_flags_then_passthrough() {
        let cfg = config(
            &["--force-dark-mode"],
            &[("profile", "default"), ("window_size", "1280x800"), ("kiosk", "true")],
        );
        let args = WebBrowserTransporter::launch_plan("chrome.exe", "https://e.com", &cfg).unwrap();
        assert_eq!(
            args,
            vec![
                "--app=https://e.com",
                "--window-size=1280,800",
                "--kiosk",
                "--new-window",
                "--force-dark-mode"
            ]
        );
    }

    #[test]
    fn gecko_requires_kiosk_and_ignores_window_size() {
        // Without kiosk: refused with guidance, never a plain tab window.
        let err = WebBrowserTransporter::launch_plan(
            "/usr/bin/firefox",
            "https://e.com",
            &config(&[], &[]),
        )
        .unwrap_err();
        assert!(err.to_string().contains("kiosk=true"), "{err}");

        // With kiosk: -kiosk + url + own-instance; window_size warned away.
        let cfg = config(&[], &[("kiosk", "true"), ("window_size", "1x1")]);
        let args =
            WebBrowserTransporter::launch_plan("firefox", "https://e.com", &cfg).unwrap();
        assert_eq!(args, vec!["-kiosk", "https://e.com", "--new-instance"]);
    }

    #[test]
    fn invalid_params_fail_with_precise_errors() {
        let bad_size = config(&[], &[("window_size", "wide")]);
        assert!(matches!(
            WebBrowserTransporter::launch_plan("chromium", "https://e.com", &bad_size),
            Err(AppError::InvalidResolutionFormat(_))
        ));
        let bad_kiosk = config(&[], &[("kiosk", "maybe")]);
        assert!(matches!(
            WebBrowserTransporter::launch_plan("chromium", "https://e.com", &bad_kiosk),
            Err(AppError::InvalidParamValue { key, .. }) if key == "kiosk"
        ));
    }

    #[test]
    fn flags_unknown_params_only() {
        let cfg = config(&[], &[("browser_path", "/x"), ("window_siez", "1x1")]);
        assert_eq!(
            WebBrowserTransporter::unknown_param_keys(&cfg.params),
            vec!["window_siez"]
        );
    }

    #[test]
    fn resolution_checks_paths_but_not_bare_names_outside_path() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("my-browser");
        std::fs::write(&fake, "#!/bin/sh\n").unwrap();
        assert!(resolves(fake.to_str().unwrap()));
        assert!(!resolves(dir.path().join("absent").to_str().unwrap()));
    }
}
