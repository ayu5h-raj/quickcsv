//! JSON detection and formatting utilities

/// Check if a string looks like JSON (starts with { or [)
/// Optimized: only checks first/last characters, doesn't scan entire string
pub fn looks_like_json(s: &str) -> bool {
    // For huge strings, only check the boundaries (first 100 and last 100 chars)
    // This avoids O(n) trim() on megabyte strings
    let len = s.len();
    if len == 0 {
        return false;
    }

    // Find first non-whitespace
    let first_char = s.bytes().take(100).find(|&b| !b.is_ascii_whitespace());
    // Find last non-whitespace
    let last_char = s
        .bytes()
        .rev()
        .take(100)
        .find(|&b| !b.is_ascii_whitespace());

    matches!(
        (first_char, last_char),
        (Some(b'{'), Some(b'}')) | (Some(b'['), Some(b']'))
    )
}

/// Try to parse and format JSON, returns None if invalid
pub fn format_json(s: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(s).ok()?;
    serde_json::to_string_pretty(&value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_looks_like_json_object() {
        assert!(looks_like_json(r#"{"key": "value"}"#));
        assert!(looks_like_json(r#"  {"key": "value"}  "#)); // With whitespace
    }

    #[test]
    fn test_looks_like_json_array() {
        assert!(looks_like_json(r#"[1, 2, 3]"#));
        assert!(looks_like_json(r#"  [1, 2, 3]  "#));
    }

    #[test]
    fn test_looks_like_json_not_json() {
        assert!(!looks_like_json("hello world"));
        assert!(!looks_like_json("123"));
        assert!(!looks_like_json("{incomplete"));
        assert!(!looks_like_json("[incomplete"));
    }

    #[test]
    fn test_looks_like_json_empty() {
        assert!(!looks_like_json(""));
        assert!(!looks_like_json("   ")); // Only whitespace
    }

    #[test]
    fn test_looks_like_json_nested() {
        assert!(looks_like_json(r#"{"nested": {"key": "value"}}"#));
        assert!(looks_like_json(r#"[[1, 2], [3, 4]]"#));
    }

    #[test]
    fn test_format_json_valid() {
        let json = r#"{"name":"John","age":30}"#;
        let result = format_json(json);
        assert!(result.is_some());
        assert!(result.unwrap().contains("\"name\": \"John\""));
    }

    #[test]
    fn test_format_json_invalid() {
        assert!(format_json("not json").is_none());
        assert!(format_json("{incomplete").is_none());
    }

    #[test]
    fn test_format_json_array() {
        let json = r#"[1, 2, 3]"#;
        let result = format_json(json);
        assert!(result.is_some());
        assert!(result.unwrap().contains("["));
    }

    #[test]
    fn test_format_json_nested() {
        let json = r#"{"outer": {"inner": "value"}}"#;
        let result = format_json(json);
        assert!(result.is_some());
        let formatted = result.unwrap();
        assert!(formatted.contains("outer"));
        assert!(formatted.contains("inner"));
    }
}
