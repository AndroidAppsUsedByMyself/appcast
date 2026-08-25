//! `web-webview` — appcast transporter plugin with an **embedded** WebView.
//!
//! Where the built-in `web-browser` backend delegates to whatever browser
//! is installed, this plugin owns its window: a native WebView (WebKitGTK
//! on Linux, WebView2 on Windows, WKWebView on macOS) rendered inside a
//! plain tao window — no address bar, no tabs, no browser dependency.
//!
//! All heavy dependencies live in this cdylib and never touch the appcast
//! binary; the host talks to us exclusively through the C ABI v1 contract
//! from `appcast-plugin`.
//!
//! Addressing schema: `target` = http(s) URL; `app` unused.
//! Params:
//! - `window_size`: `<WxH>` initial logical size (default: 1024x768)
//! - `title`: window title (default: `appcast — <url>`)
//!
//! Platform notes: the GUI event loop must own one dedicated thread. Linux
//! and Windows tolerate that thread not being process-main; macOS requires
//! the main thread, so launching from a worker pool there may abort —
//! documented limitation until the host grows a main-thread bridge.

use std::collections::HashMap;

use appcast_plugin::{
    export_appcast_transporter, ConfigSnapshot, ListedApp, SimpleTransporter,
};
// run_return returns control instead of tao's run(), which ends with
// process::exit and skips ordered teardown of the webview/window.
use tao::platform::run_return::EventLoopExtRunReturn;

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;

// The host invokes us on a pool worker, never process-main. Linux and
// Windows permit dedicated-thread event loops via the *_any_thread
// constructors; macOS keeps the strict main-thread rule (documented
// limitation).
#[cfg(target_os = "linux")]
use tao::platform::unix::EventLoopBuilderExtUnix;
#[cfg(target_os = "windows")]
use tao::platform::windows::EventLoopBuilderExtWindows;

/// Platform-appropriate event loop construction for our session thread.
fn event_loop() -> EventLoop<()> {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        let mut builder = tao::event_loop::EventLoopBuilder::<()>::new();
        builder.with_any_thread(true);
        builder.build()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        EventLoop::new()
    }
}
use wry::WebViewBuilder;

/// Fallback window geometry when no `window_size` param is given.
const DEFAULT_WINDOW: (f64, f64) = (1024.0, 768.0);

struct WebViewTransporter;

impl WebViewTransporter {
    /// The addressing schema of this plugin: exactly one slot, http(s) only.
    fn validate_url(url: &str) -> Result<(), String> {
        let lower = url.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            Ok(())
        } else {
            Err(format!(
                "invalid URL `{url}`: web-webview expects an http(s) URL such as \
                 `https://example.com`"
            ))
        }
    }

    fn parse_size(config: &ConfigSnapshot) -> Result<(f64, f64), String> {
        match config.param("window_size") {
            None => Ok(DEFAULT_WINDOW),
            Some(value) => {
                let invalid =
                    || format!("invalid window_size `{value}`: expected `<W>x<H>`");
                let (w, h) = value.split_once('x').ok_or_else(invalid)?;
                let w: f64 = w.trim().parse().map_err(|_| invalid())?;
                let h: f64 = h.trim().parse().map_err(|_| invalid())?;
                if w <= 0.0 || h <= 0.0 {
                    return Err(invalid());
                }
                Ok((w, h))
            }
        }
    }

    /// Build the window + webview pair and pump events until the user
    /// closes the window.
    fn session(url: &str, title: &str, size: (f64, f64)) -> Result<(), String> {
        let mut event_loop = event_loop();
        #[cfg(target_os = "linux")]
        use tao::platform::unix::{WindowBuilderExtUnix, WindowExtUnix};
        let window = WindowBuilder::new()
            .with_default_vbox(true)
            .with_title(title.to_string())
        .with_inner_size(LogicalSize::new(size.0, size.1))
        .build(&event_loop)
        .map_err(|e| format!("window: {e}"))?;

        // Linux: attach into tao's own vbox — packing the webview straight
        // into the top-level window collides with tao's internal layout and
        // yields a blank surface. wry packs into a GtkBox with
        // pack_start(fill=true), so it fills and tracks resizes.
        #[allow(unused_variables)]
        let window_ref = &window;
        let _webview = {
            #[cfg(target_os = "linux")]
            {
                use wry::WebViewBuilderExtUnix;
                let vbox = window_ref.default_vbox().ok_or("no default vbox")?;
                WebViewBuilder::new()
                    .with_url(url)
                    .build_gtk(vbox)
                    .map_err(|e| format!("webview: {e}"))?
            }
            #[cfg(not(target_os = "linux"))]
            {
                WebViewBuilder::new()
                    .with_url(url)
                    .build(window_ref)
                    .map_err(|e| format!("webview: {e}"))?
            }
        };

        // CloseRequested MUST be handled explicitly: the default Wait
        // policy ignores it and leaves a zombie session behind. Set
        // APPCAST_WEBVIEW_DEBUG=1 for per-event tracing.
        let debug_events = std::env::var_os("APPCAST_WEBVIEW_DEBUG").is_some();
        let exit_code = event_loop.run_return(move |event, _, control_flow| {
            *control_flow = ControlFlow::Wait;
            match &event {
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                } => {
                    if debug_events {
                        eprintln!("web-webview: CloseRequested -> exiting");
                    }
                    *control_flow = ControlFlow::Exit;
                }
                other => {
                    if debug_events {
                        match other {
                            Event::MainEventsCleared => {}
                            Event::WindowEvent { event, .. } => {
                                eprintln!("web-webview: win-ev {event:?}")
                            }
                            ev => eprintln!("web-webview: ev {ev:?}"),
                        }
                    }
                }
            }
        });

        // Teardown policy: deliberately NOT dropping the webview/window
        // here. Widget disposal after the loop stopped needs main-context
        // iterations we no longer pump, and has been observed to deadlock
        // the session thread (notably on Wayland) — the host then hangs
        // after "window closed". Leaking is safe-by-design for a CLI: the
        // OS reclaims everything when appcast exits.
        if debug_events {
            eprintln!("web-webview: loop exited with code {exit_code}; leaking gui handles");
        }
        std::mem::forget(_webview);
        std::mem::forget(window);

        if exit_code == 0 {
            Ok(())
        } else {
            Err(format!("display connection lost (code {exit_code})"))
        }
    }
}

impl SimpleTransporter for WebViewTransporter {
    fn name(&self) -> &'static str {
        "web-webview"
    }

    fn run(&self, config: ConfigSnapshot) -> Result<(), String> {
        const USAGE: &str = "appcast run web-webview <https-url>";
        let url = config
            .target
            .clone()
            .ok_or_else(|| format!("missing URL in the address slot; usage: {USAGE}"))?;
        Self::validate_url(&url)?;

        for key in config.params.keys() {
            if !matches!(key.as_str(), "window_size" | "title") {
                eprintln!(
                    "web-webview: ignoring unknown param `{key}` (known: window_size, title)"
                );
            }
        }

        let title = config
            .params
            .get("title")
            .cloned()
            .unwrap_or_else(|| format!("appcast — {url}"));
        let size = Self::parse_size(&config)?;

        // WebKitGTK's DMABUF renderer blanks out on GPUs/drivers without
        // proper DRI3 (VMs, Xvfb, some NVIDIA setups). Default to the
        // software-safe path unless the user explicitly opted otherwise.
        #[cfg(target_os = "linux")]
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }

        // The GTK/WinUI event loops dislike foreign threads re-entering;
        // give the session one dedicated thread of its own and block this
        // worker on it. A panic inside becomes our error via join.
        let gui = std::thread::Builder::new()
            .name("appcast-webview".into())
            .spawn(move || Self::session(&url, &title, size))
            .map_err(|e| format!("spawn GUI thread: {e}"))?;
        gui.join()
            .map_err(|_| "GUI thread panicked".to_string())?
    }

    fn list_apps(
        &self,
        _target: &str,
        _params: &HashMap<String, String>,
    ) -> Result<Vec<ListedApp>, String> {
        // Semantic emptiness: a URL has nothing enumerable. (Forks could
        // surface saved bookmarks here.)
        Ok(Vec::new())
    }
}

export_appcast_transporter!(WebViewTransporter);
