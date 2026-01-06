//! Tab state management for multi-file support

use crate::state::{
    ColumnState, FilterCondition, FilterState, GoToRowState, JsonViewerState, LoadState,
    RowDetailState, SearchState, SortDirection, SortState,
};
use parking_lot::RwLock;
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};

/// State for a single tab (one CSV file)
pub struct TabState {
    /// Shared state with background thread (CSV data)
    pub state: Arc<RwLock<crate::state::SharedState>>,
    /// File path (desktop) or file name (web)
    pub file_path: String,
    /// Display name (filename only)
    pub file_name: String,
    /// Vertical scroll offset
    pub scroll_y: f32,
    /// Horizontal scroll offset
    pub scroll_x: f32,
    /// Column widths (auto-sized or user-adjusted)
    pub column_widths: Vec<f32>,
    /// Row cache for rendering (row_index -> parsed fields)
    pub row_cache: HashMap<usize, Vec<String>>,
    /// Last visible row range for cache invalidation
    pub last_visible_range: (usize, usize),
    /// Search state
    pub search: SearchState,
    /// JSON viewer popup state
    pub json_viewer: JsonViewerState,
    /// Column sort state
    pub sort_state: SortState,
    /// Go to row dialog state
    pub go_to_row: GoToRowState,
    /// Row detail popup state
    pub row_detail: RowDetailState,
    /// Column state (visibility, order, undo/redo)
    pub column_state: ColumnState,
    /// Filter state for column filtering
    pub filter_state: FilterState,
    /// Cached filtered row indices (None = no filter, Some = indices that pass filter)
    pub filtered_indices: Option<Vec<usize>>,
    /// Total row count (before filtering) for status bar
    pub total_row_count: usize,
    /// Filter version (increments when filter changes, used to trigger recompute)
    pub filter_version: u32,
    /// Last computed filter version (to detect when recompute is needed)
    pub last_filter_version: u32,
    /// Track previous sorting state to detect completion
    pub was_sorting: bool,
    /// Channel to receive filtered indices from background thread
    #[allow(clippy::type_complexity)]
    pub filter_receiver: Option<
        mpsc::Receiver<(
            Vec<usize>,
            HashMap<usize, FilterCondition>,
            Option<usize>,
            SortDirection,
            std::time::Duration,
        )>,
    >,
    /// Whether filtering is currently in progress
    pub is_filtering: Arc<AtomicBool>,
    /// Optimization: Filters that were applied to generate current result
    pub applied_filters: HashMap<usize, FilterCondition>,
    /// Optimization: Sort column used to generate current result
    pub applied_sort_column: Option<usize>,
    /// Optimization: Sort direction used to generate current result
    pub applied_sort_direction: SortDirection,
    /// Duration of last filter operation (for display)
    pub filter_duration: Option<std::time::Duration>,
}

impl TabState {
    /// Create a new empty tab
    pub fn new_empty() -> Self {
        Self {
            state: Arc::new(RwLock::new(crate::state::SharedState::default())),
            file_path: String::new(),
            file_name: String::new(),
            scroll_y: 0.0,
            scroll_x: 0.0,
            column_widths: Vec::new(),
            row_cache: HashMap::new(),
            last_visible_range: (0, 0),
            search: SearchState::default(),
            json_viewer: JsonViewerState::default(),
            sort_state: SortState::default(),
            go_to_row: GoToRowState::default(),
            row_detail: RowDetailState::default(),
            column_state: ColumnState::default(),
            filter_state: FilterState::default(),
            filtered_indices: None,
            total_row_count: 0,
            filter_version: 0,
            last_filter_version: 0,
            was_sorting: false,
            filter_receiver: None,
            is_filtering: Arc::new(AtomicBool::new(false)),
            applied_filters: HashMap::new(),
            applied_sort_column: None,
            applied_sort_direction: SortDirection::None,
            filter_duration: None,
        }
    }

    /// Create a new tab from file path
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_path(path: PathBuf) -> Self {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled")
            .to_string();
        let file_path = path.to_string_lossy().to_string();

        Self {
            state: Arc::new(RwLock::new(crate::state::SharedState::default())),
            file_path,
            file_name,
            scroll_y: 0.0,
            scroll_x: 0.0,
            column_widths: Vec::new(),
            row_cache: HashMap::new(),
            last_visible_range: (0, 0),
            search: SearchState::default(),
            json_viewer: JsonViewerState::default(),
            sort_state: SortState::default(),
            go_to_row: GoToRowState::default(),
            row_detail: RowDetailState::default(),
            column_state: ColumnState::default(),
            filter_state: FilterState::default(),
            filtered_indices: None,
            total_row_count: 0,
            filter_version: 0,
            last_filter_version: 0,
            was_sorting: false,
            filter_receiver: None,
            is_filtering: Arc::new(AtomicBool::new(false)),
            applied_filters: HashMap::new(),
            applied_sort_column: None,
            applied_sort_direction: SortDirection::None,
            filter_duration: None,
        }
    }

    /// Create a new tab from file name (web)
    #[cfg(target_arch = "wasm32")]
    pub fn from_name(name: String) -> Self {
        Self {
            state: Arc::new(RwLock::new(crate::state::SharedState::default())),
            file_path: name.clone(),
            file_name: name,
            scroll_y: 0.0,
            scroll_x: 0.0,
            column_widths: Vec::new(),
            row_cache: HashMap::new(),
            last_visible_range: (0, 0),
            search: SearchState::default(),
            json_viewer: JsonViewerState::default(),
            sort_state: SortState::default(),
            go_to_row: GoToRowState::default(),
            row_detail: RowDetailState::default(),
            column_state: ColumnState::default(),
            filter_state: FilterState::default(),
            filtered_indices: None,
            total_row_count: 0,
            filter_version: 0,
            last_filter_version: 0,
            was_sorting: false,
            filter_receiver: None,
            is_filtering: Arc::new(AtomicBool::new(false)),
            applied_filters: HashMap::new(),
            applied_sort_column: None,
            applied_sort_direction: SortDirection::None,
            filter_duration: None,
        }
    }

    /// Check if tab is empty (no file loaded)
    pub fn is_empty(&self) -> bool {
        let state = self.state.read();
        matches!(state.load_state, LoadState::Empty)
    }
}
