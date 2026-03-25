//! Google Analytics 4 (GA4) Measurement Protocol tracking for desktop app
//!
//! This module sends analytics events to Google Analytics using the Measurement Protocol API.
//! All tracking happens in background threads and failures are silent to avoid affecting app performance.
//!
//! **Offline Support**: Events are queued locally when offline and automatically retried when online.
//! Queue is limited to 100 events to prevent unbounded storage growth.
//!
//! To get your API Secret:
//! 1. Go to https://analytics.google.com/
//! 2. Admin → Property → Data Streams → [Your Stream] → Measurement Protocol API secrets
//! 3. Create a new secret and copy the value

#[cfg(not(target_arch = "wasm32"))]
use std::fs;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{Read, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use std::thread;

/// Google Analytics Measurement ID (same as web version)
const MEASUREMENT_ID: &str = "G-02EQ3MT9HS";

/// Google Analytics Measurement Protocol API Secret
/// Get this from: Admin → Property → Data Streams → Measurement Protocol API secrets
const API_SECRET: &str = "tluu1LcBSe6Yx8hYUtuM4A";

/// GA4 Measurement Protocol endpoint
const GA4_ENDPOINT: &str = "https://www.google-analytics.com/mp/collect";

/// Maximum number of events to queue when offline (prevents unbounded storage)
const MAX_QUEUE_SIZE: usize = 100;

/// Get or create a persistent client ID for this installation
/// Client ID is stored in the app's config directory and persists across sessions
#[cfg(not(target_arch = "wasm32"))]
fn get_client_id() -> String {
    let config_dir = match dirs::config_dir() {
        Some(dir) => dir.join("quickcsv"),
        None => return generate_client_id(), // Fallback if config dir unavailable
    };

    // Create config directory if it doesn't exist
    if fs::create_dir_all(&config_dir).is_err() {
        return generate_client_id(); // Fallback if directory creation fails
    }

    let client_id_path = config_dir.join("analytics_client_id.txt");

    // Try to read existing client ID
    if let Ok(client_id) = fs::read_to_string(&client_id_path) {
        let trimmed = client_id.trim();
        if !trimmed.is_empty() && is_valid_uuid(trimmed) {
            return trimmed.to_string();
        }
    }

    // Generate new client ID and save it
    let new_client_id = generate_client_id();
    if let Ok(mut file) = fs::File::create(&client_id_path) {
        let _ = file.write_all(new_client_id.as_bytes());
    }
    new_client_id
}

/// Generate a random UUID v4-style client ID
/// Format: xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx
#[cfg(not(target_arch = "wasm32"))]
fn generate_client_id() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    // Generate a pseudo-random UUID-like string
    // This is not cryptographically secure but sufficient for analytics client ID
    let mut hasher = DefaultHasher::new();
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .hash(&mut hasher);
    std::process::id().hash(&mut hasher);

    let hash = hasher.finish();
    let mut bytes = [0u8; 16];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = ((hash >> (i * 4)) & 0xFF) as u8;
    }

    // Format as UUID v4
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        u16::from_le_bytes([bytes[4], bytes[5]]),
        u16::from_le_bytes([bytes[6], bytes[7]]) & 0x0FFF,
        (u16::from_le_bytes([bytes[8], bytes[9]]) & 0x3FFF) | 0x8000,
        u64::from_le_bytes([
            bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15], 0, 0
        ])
    )
}

/// Check if a string is a valid UUID format
#[cfg(not(target_arch = "wasm32"))]
fn is_valid_uuid(s: &str) -> bool {
    // Simple check: should be 36 chars with dashes in right places
    s.len() == 36
        && s.chars().nth(8) == Some('-')
        && s.chars().nth(13) == Some('-')
        && s.chars().nth(18) == Some('-')
        && s.chars().nth(23) == Some('-')
        && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Get the path to the event queue file
#[cfg(not(target_arch = "wasm32"))]
fn get_queue_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("quickcsv").join("analytics_queue.json"))
}

/// Load queued events from disk
#[cfg(not(target_arch = "wasm32"))]
fn load_event_queue() -> Vec<serde_json::Value> {
    if let Some(queue_path) = get_queue_path() {
        if let Ok(mut file) = fs::File::open(&queue_path) {
            let mut contents = String::new();
            if file.read_to_string(&mut contents).is_ok() {
                if let Ok(events) = serde_json::from_str::<Vec<serde_json::Value>>(&contents) {
                    return events;
                }
            }
        }
    }
    Vec::new()
}

/// Save queued events to disk
#[cfg(not(target_arch = "wasm32"))]
fn save_event_queue(events: &[serde_json::Value]) {
    if let Some(queue_path) = get_queue_path() {
        // Ensure directory exists
        if let Some(parent) = queue_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if let Ok(json) = serde_json::to_string(events) {
            let _ = fs::write(&queue_path, json);
        }
    }
}

/// Queue an event for later retry (when offline)
#[cfg(not(target_arch = "wasm32"))]
fn queue_event(payload: serde_json::Value) {
    let mut events = load_event_queue();

    // Limit queue size - remove oldest events if at limit
    if events.len() >= MAX_QUEUE_SIZE {
        events.remove(0);
    }

    events.push(payload);
    save_event_queue(&events);
}

/// Try to send an event, queue it if offline
/// Returns Ok(()) if sent successfully, Err(()) if queued
#[cfg(not(target_arch = "wasm32"))]
fn send_or_queue_event(payload: serde_json::Value) -> Result<(), ()> {
    let url = format!("{GA4_ENDPOINT}?measurement_id={MEASUREMENT_ID}&api_secret={API_SECRET}");

    // Try to send the request
    // Convert payload to JSON string
    let json_str = match serde_json::to_string(&payload) {
        Ok(s) => s,
        Err(_) => {
            // Failed to serialize - queue anyway
            queue_event(payload);
            return Err(());
        }
    };

    // Use ureq's send method - String implements AsSendBody
    let result = ureq::post(&url)
        .header("Content-Type", "application/json")
        .send(&json_str);

    // Debug: Log the payload structure (first 200 chars) for troubleshooting
    #[cfg(debug_assertions)]
    {
        let preview = if json_str.len() > 200 {
            format!("{}...", &json_str[..200])
        } else {
            json_str.clone()
        };
        eprintln!("[Analytics] Sending event payload: {preview}");
    }

    if result.is_err() {
        // Failed (likely offline) - queue for retry
        #[cfg(debug_assertions)]
        eprintln!("[Analytics] Failed to send event, queuing for retry");
        queue_event(payload);
        Err(())
    } else {
        // Success - no need to queue
        #[cfg(debug_assertions)]
        eprintln!("[Analytics] Event sent successfully");
        Ok(())
    }
}

/// Retry sending queued events (called on app start)
#[cfg(not(target_arch = "wasm32"))]
pub fn retry_queued_events() {
    let events = load_event_queue();
    if events.is_empty() {
        return;
    }

    thread::spawn(move || {
        let mut remaining_events = Vec::new();

        for event in events {
            if send_or_queue_event(event.clone()).is_err() {
                // Still offline or failed - keep in queue
                remaining_events.push(event);
            } else {
                // Successfully sent - don't add to remaining
            }
        }

        // Save remaining events (if any)
        if !remaining_events.is_empty() {
            save_event_queue(&remaining_events);
        } else {
            // All sent successfully - delete queue file
            if let Some(queue_path) = get_queue_path() {
                let _ = fs::remove_file(queue_path);
            }
        }
    });
}

/// Get user properties for analytics
#[cfg(not(target_arch = "wasm32"))]
fn get_user_properties() -> serde_json::Value {
    use crate::update::CURRENT_VERSION;

    // Get OS information
    let os = if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        "Unknown"
    };

    // Get architecture
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };

    // GA4 Measurement Protocol requires user properties to be objects with "value" field
    serde_json::json!({
        "app_version": {
            "value": CURRENT_VERSION
        },
        "os": {
            "value": os
        },
        "arch": {
            "value": arch
        },
        "platform": {
            "value": "desktop"
        }
    })
}

/// Send an event to Google Analytics (non-blocking, runs in background thread)
/// If offline, the event will be queued and retried later
#[cfg(not(target_arch = "wasm32"))]
fn track_event_internal(event_name: &str, params: Option<serde_json::Value>) {
    let client_id = get_client_id();
    let event_name = event_name.to_string();
    let user_properties = get_user_properties();

    thread::spawn(move || {
        // Build the event payload
        let mut event = serde_json::json!({
            "name": event_name,
        });

        if let Some(p) = params {
            if let Some(obj) = p.as_object() {
                if !obj.is_empty() {
                    event["params"] = p;
                }
            }
        }

        // CRITICAL: GA4 requires engagement_time_msec for events to count toward Active Users
        // Set to 100ms (minimum engagement) if not already set in params
        if !event["params"].is_object()
            || !event["params"]
                .as_object()
                .unwrap()
                .contains_key("engagement_time_msec")
        {
            if !event["params"].is_object() {
                event["params"] = serde_json::json!({});
            }
            event["params"]["engagement_time_msec"] = serde_json::json!(100);
        }

        let payload = serde_json::json!({
            "client_id": client_id,
            "user_properties": user_properties,
            "events": [event]
        });

        let _ = send_or_queue_event(payload);
    });
}

/// Track app start event
/// Also retries any queued events from previous sessions
#[cfg(not(target_arch = "wasm32"))]
pub fn track_app_start() {
    // Retry queued events from previous sessions
    retry_queued_events();

    // Track current app start
    track_event_internal("app_start", None);
}

/// Track file open event
#[cfg(not(target_arch = "wasm32"))]
pub fn track_file_open(row_count: usize, column_count: usize) {
    let params = serde_json::json!({
        "row_count": row_count,
        "column_count": column_count,
    });
    track_event_internal("file_open", Some(params));
}

/// Track filter applied event
#[cfg(not(target_arch = "wasm32"))]
pub fn track_filter_applied(filter_count: usize) {
    let params = serde_json::json!({
        "filter_count": filter_count,
    });
    track_event_internal("filter_applied", Some(params));
}

/// Track search performed event
#[cfg(not(target_arch = "wasm32"))]
pub fn track_search_performed() {
    track_event_internal("search_performed", None);
}

/// Track column action (hide, show, or reorder)
#[cfg(not(target_arch = "wasm32"))]
pub fn track_column_action(action: &str) {
    let params = serde_json::json!({
        "action": action,
    });
    track_event_internal("column_action", Some(params));
}

/// Track export action (if implemented in future)
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)] // Reserved for future export feature
pub fn track_export_action(format: &str) {
    let params = serde_json::json!({
        "format": format,
    });
    track_event_internal("export_action", Some(params));
}

// WASM implementation - calls JavaScript gtag functions
#[cfg(target_arch = "wasm32")]
fn call_js_track_event(event_name: &str, params: Option<serde_json::Value>) {
    use wasm_bindgen::prelude::*;
    use web_sys::window;

    if let Some(window) = window() {
        // Get the trackGAEvent function from window
        if let Ok(js_func) = js_sys::Reflect::get(&window, &JsValue::from_str("trackGAEvent")) {
            if js_func.is_function() {
                let func = js_sys::Function::from(js_func);
                let event_name_js = JsValue::from_str(event_name);

                // Convert params to JS object if provided
                let params_js = if let Some(p) = params {
                    // Convert serde_json::Value to JsValue
                    serde_wasm_bindgen::to_value(&p).unwrap_or(JsValue::NULL)
                } else {
                    JsValue::NULL
                };

                // Call the JavaScript function
                let _ = func.call2(&window, &event_name_js, &params_js);
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub fn track_app_start() {
    call_js_track_event("app_start", None);
}

#[cfg(target_arch = "wasm32")]
pub fn retry_queued_events() {
    // No-op for web - JavaScript handles this automatically
}

#[cfg(target_arch = "wasm32")]
pub fn track_file_open(row_count: usize, column_count: usize) {
    let params = serde_json::json!({
        "row_count": row_count,
        "column_count": column_count,
    });
    call_js_track_event("file_open", Some(params));
}

#[cfg(target_arch = "wasm32")]
pub fn track_filter_applied(filter_count: usize) {
    let params = serde_json::json!({
        "filter_count": filter_count,
    });
    call_js_track_event("filter_applied", Some(params));
}

#[cfg(target_arch = "wasm32")]
pub fn track_search_performed() {
    call_js_track_event("search_performed", None);
}

#[cfg(target_arch = "wasm32")]
pub fn track_column_action(action: &str) {
    let params = serde_json::json!({
        "action": action,
    });
    call_js_track_event("column_action", Some(params));
}

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)] // Reserved for future export feature
pub fn track_export_action(format: &str) {
    let params = serde_json::json!({
        "format": format,
    });
    call_js_track_event("export_action", Some(params));
}
