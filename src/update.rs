//! Update checker functionality
//!
//! This module handles checking for updates from GitHub releases.

#[cfg(not(target_arch = "wasm32"))]
use eframe::egui;
use parking_lot::RwLock;
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::AtomicBool;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::thread;

/// Current version from Cargo.toml
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Update checker state
pub struct UpdateState {
    /// Latest version available (if checked)
    pub latest_version: Arc<RwLock<Option<String>>>,
    /// Whether an update is available
    pub update_available: Arc<AtomicBool>,
    /// Whether the update banner has been dismissed
    pub dismissed: bool,
    /// Whether check has been initiated
    pub check_initiated: bool,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            latest_version: Arc::new(RwLock::new(None)),
            update_available: Arc::new(AtomicBool::new(false)),
            dismissed: false,
            check_initiated: false,
        }
    }
}

/// Check for updates from GitHub releases (runs in background thread)
#[cfg(not(target_arch = "wasm32"))]
pub fn check_for_updates(
    latest_version: Arc<RwLock<Option<String>>>,
    update_available: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    thread::spawn(move || {
        // Call GitHub API to get latest release
        let url = "https://api.github.com/repos/ayu5h-raj/quickcsv/releases/latest";

        let result = ureq::get(url)
            .header("User-Agent", "QuickCSV-Update-Checker")
            .call();

        if let Ok(response) = result {
            if let Ok(body) = response.into_body().read_to_string() {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                    if let Some(tag) = json.get("tag_name").and_then(|v| v.as_str()) {
                        // Remove 'v' prefix if present
                        let version = tag.trim_start_matches('v');

                        // Compare versions
                        if version != CURRENT_VERSION && is_newer_version(version, CURRENT_VERSION)
                        {
                            *latest_version.write() = Some(version.to_string());
                            update_available.store(true, Ordering::SeqCst);
                            ctx.request_repaint();
                        }
                    }
                }
            }
        }
        // Silently fail if update check fails
    });
}

/// Compare version strings (simple semver comparison)
pub fn is_newer_version(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u32> { v.split('.').filter_map(|s| s.parse().ok()).collect() };

    let latest_parts = parse(latest);
    let current_parts = parse(current);

    for (l, c) in latest_parts.iter().zip(current_parts.iter()) {
        if l > c {
            return true;
        } else if l < c {
            return false;
        }
    }

    // If all compared parts are equal, newer if latest has more parts
    latest_parts.len() > current_parts.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer_version() {
        assert!(is_newer_version("0.4.0", "0.3.2"));
        assert!(is_newer_version("1.0.0", "0.9.9"));
        assert!(is_newer_version("0.3.10", "0.3.9"));
        assert!(!is_newer_version("0.3.2", "0.3.2")); // Same version
        assert!(!is_newer_version("0.3.1", "0.3.2")); // Older version
        assert!(!is_newer_version("0.2.0", "0.3.0")); // Older major
    }

    #[test]
    fn test_is_newer_version_edge_cases() {
        // Test with different length versions
        assert!(is_newer_version("1.0.0.1", "1.0.0")); // More parts = newer
        assert!(!is_newer_version("1.0.0", "1.0.0.1")); // Fewer parts = older

        // Test with two-part versions
        assert!(is_newer_version("2.0", "1.9"));
        assert!(!is_newer_version("1.9", "2.0"));
    }
}
