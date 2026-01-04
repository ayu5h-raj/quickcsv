//! Dialog state structs

use std::collections::HashSet;

/// JSON viewer popup state
#[derive(Default)]
pub struct JsonViewerState {
    /// Whether the popup is open
    pub open: bool,
    /// Raw content from the cell
    pub raw_content: String,
    /// Formatted JSON (if valid)
    pub formatted_content: String,
    /// Whether the content is valid JSON
    pub is_valid_json: bool,
    /// Row index of the cell
    pub row: usize,
    /// Column index of the cell
    pub col: usize,
    /// Column name for display
    pub column_name: String,
}

/// Go to row dialog state
#[derive(Default)]
pub struct GoToRowState {
    /// Whether the dialog is open
    pub open: bool,
    /// Input text (row number)
    pub input: String,
    /// Whether to focus the input field
    pub focus_input: bool,
    /// Row to scroll to (set when confirmed)
    pub scroll_to_row: Option<usize>,
    /// Row to highlight (after jumping)
    pub highlight_row: Option<usize>,
}

/// Row detail popup state
#[derive(Default)]
pub struct RowDetailState {
    /// Whether the popup is open
    pub open: bool,
    /// Row index being displayed
    pub row_index: usize,
    /// Parsed fields for this row
    pub fields: Vec<String>,
    /// Column headers
    pub headers: Vec<String>,
    /// Which fields are expanded (for large values)
    pub expanded_fields: HashSet<usize>,
}
