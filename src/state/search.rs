//! Search state for find functionality

use parking_lot::RwLock;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Search execution state
#[derive(Clone, Copy, PartialEq, Debug, Default)]
#[allow(dead_code)]
pub enum SearchStatus {
    #[default]
    Idle,
    Searching,
    Complete,
    Cancelled,
}

/// Maximum rows to store for navigation (keeps memory bounded)
pub const MAX_NAV_ROWS: usize = 10000;

/// Shared search results (updated by background thread)
///
/// Design: We DON'T store all matches. Instead:
/// - Count total matches for display
/// - Store limited navigation rows (first N rows with matches)
/// - Highlighting is done on-the-fly during rendering (viewport only)
pub struct SearchResults {
    /// Current search status
    pub status: SearchStatus,
    /// Number of rows searched so far
    pub rows_searched: usize,
    /// Total rows to search
    pub total_rows: usize,
    /// Total number of matching cells found (for display only)
    pub total_match_count: usize,
    /// Row indices that contain matches (limited for navigation)
    /// Only stores first MAX_NAV_ROWS unique rows
    pub navigation_rows: Vec<usize>,
    /// Whether we've hit the navigation row limit
    pub nav_limit_reached: bool,
}

impl Default for SearchResults {
    fn default() -> Self {
        Self {
            status: SearchStatus::Idle,
            rows_searched: 0,
            total_rows: 0,
            total_match_count: 0,
            navigation_rows: Vec::new(),
            nav_limit_reached: false,
        }
    }
}

/// Search state for find functionality
pub struct SearchState {
    /// Whether search bar is visible
    pub visible: bool,
    /// Whether to focus the input field (set when opening search)
    pub focus_input: bool,
    /// Current search query (in the text field) (set when opening search)
    /// Current search query (in the text field)
    pub query: String,
    /// The active/confirmed search query (lowercase, for matching)
    pub active_query: String,
    /// Current navigation index (for prev/next through rows)
    pub current_index: usize,
    /// Row to scroll to (set when navigating matches)
    pub scroll_to_row: Option<usize>,
    /// Shared search results (updated by background thread)
    pub results: Arc<RwLock<SearchResults>>,
    /// Flag to cancel ongoing search
    pub cancel_flag: Arc<AtomicBool>,
    /// Search history (most recent at the end)
    pub history: Vec<String>,
    /// Current position in history (None = not browsing history)
    pub history_index: Option<usize>,
    /// Temporary storage for current query when browsing history
    pub history_temp_query: String,
}

impl SearchState {
    /// Clear search state (but keep history)
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.visible = false;
        self.query.clear();
        self.active_query.clear();
        self.current_index = 0;
        self.scroll_to_row = None;
        self.results.write().status = SearchStatus::Idle;
        self.results.write().total_match_count = 0;
        self.results.write().navigation_rows.clear();
        self.cancel_flag
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            visible: false,
            focus_input: false,
            query: String::new(),
            active_query: String::new(),
            current_index: 0,
            scroll_to_row: None,
            results: Arc::new(RwLock::new(SearchResults::default())),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            history: Vec::new(),
            history_index: None,
            history_temp_query: String::new(),
        }
    }
}
