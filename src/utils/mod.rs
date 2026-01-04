//! Utility functions for formatting and display
//!
//! This module contains pure utility functions that have no dependencies
//! on the rest of the application.

mod format;
mod json;

pub use format::{format_file_size, format_number, truncate_for_display};
pub use json::{format_json, looks_like_json};
