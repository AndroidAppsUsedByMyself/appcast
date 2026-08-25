//! Shared session lifecycle for backends that spawn a long-lived child
//! process (adb-scrcpy, web-browser, ...): uniform Ctrl+C handling and
//! mandatory teardown regardless of exit path.

use tokio::process::Child;
use tracing::debug;

/// Block until the child exits naturally or the user hits Ctrl+C.
/// Returns a human-readable reason for the session-finished log line.
pub(crate) async fn wait_or_ctrl_c(child: &mut Child) -> String {
    tokio::select! {
        status = child.wait() => match status {
            Ok(status) => format!("backend exited ({status})"),
            Err(e) => format!("backend crashed: {e}"),
        },
        _ = tokio::signal::ctrl_c() => "interrupted by Ctrl+C".to_string(),
    }
}

/// Mandatory teardown: kill any lingering process and reap it (for
/// adb-scrcpy this is what destroys the virtual display).
pub(crate) async fn reap(child: &mut Child) {
    if child.id().is_some() {
        debug!("killing lingering backend process");
        let _ = child.kill().await;
    }
    let _ = child.wait().await; // reap, ignore errors
}
