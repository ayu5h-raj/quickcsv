//! Formatting utilities for numbers and file sizes

use std::borrow::Cow;

/// Maximum length for displaying cell content before truncation.
/// Full content is available via double-click popup.
pub const MAX_DISPLAY_LEN: usize = 200;

/// Format a number with thousand separators
pub fn format_number(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// Format file size in human-readable form
pub fn format_file_size(bytes: u64) -> String {
    use humansize::{format_size, BINARY};
    format_size(bytes, BINARY)
}

/// Truncate a string for display, keeping it short for UI performance.
/// Full content is available via double-click popup.
pub fn truncate_for_display(s: &str) -> Cow<'_, str> {
    if s.len() <= MAX_DISPLAY_LEN {
        Cow::Borrowed(s)
    } else {
        // Find a safe UTF-8 boundary
        let mut end = MAX_DISPLAY_LEN;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        Cow::Owned(format!("{}…", &s[..end]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(1000000), "1,000,000");
        assert_eq!(format_number(1234567), "1,234,567");
    }

    #[test]
    fn test_format_number_edge_cases() {
        assert_eq!(format_number(1), "1");
        assert_eq!(format_number(12), "12");
        assert_eq!(format_number(123), "123");
        assert_eq!(format_number(1234), "1,234");
        assert_eq!(format_number(12345), "12,345");
        assert_eq!(format_number(123456), "123,456");
    }

    #[test]
    fn test_format_file_size() {
        assert_eq!(format_file_size(0), "0 B");
        assert_eq!(format_file_size(1023), "1023 B");
        assert_eq!(format_file_size(1024), "1 KiB");
        assert_eq!(format_file_size(1024 * 1024), "1 MiB");
        assert_eq!(format_file_size(1024 * 1024 * 1024), "1 GiB");
    }

    #[test]
    fn test_truncate_short_string() {
        let short = "hello";
        let result = truncate_for_display(short);
        assert_eq!(result.as_ref(), "hello");
    }

    #[test]
    fn test_truncate_long_string() {
        let long = "a".repeat(300);
        let result = truncate_for_display(&long);
        assert!(result.len() <= MAX_DISPLAY_LEN + 3); // +3 for "…"
        assert!(result.ends_with('…'));
    }

    #[test]
    fn test_truncate_exact_boundary() {
        let exact = "a".repeat(MAX_DISPLAY_LEN);
        let result = truncate_for_display(&exact);
        assert_eq!(result.len(), MAX_DISPLAY_LEN);
        assert!(!result.ends_with('…'));
    }

    #[test]
    fn test_truncate_utf8_boundary() {
        // '日' is 3 bytes, so truncating in the middle should be safe
        let utf8_string = "日".repeat(100); // 300 bytes
        let result = truncate_for_display(&utf8_string);
        assert!(result.len() <= MAX_DISPLAY_LEN + 3);
        // Verify it's valid UTF-8 (this would panic if not)
        let _ = result.chars().count();
    }

    #[test]
    fn test_truncate_empty_string() {
        let result = truncate_for_display("");
        assert_eq!(result.as_ref(), "");
    }
}
