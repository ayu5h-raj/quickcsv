//! Tab state management for multi-file support

use crate::state::{
    ColumnState, FilterCondition, FilterState, GoToRowState, JsonViewerState, LoadState,
    RowDetailState, SearchState, SortDirection, SortState,
};
use parking_lot::RwLock;
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

#[cfg(target_arch = "wasm32")]
static TAB_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[cfg(target_arch = "wasm32")]
fn next_tab_id() -> u64 {
    TAB_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// State for a single tab (one CSV file)
#[cfg(not(target_arch = "wasm32"))]
pub struct ExternalReloadState {
    pub headers: Option<Vec<String>>,
    pub sort_column: Option<usize>,
    pub sort_direction: SortDirection,
}

/// State for a single tab (one CSV file)
pub struct TabState {
    /// Stable identity used by asynchronous file pickers and workers
    #[cfg(target_arch = "wasm32")]
    pub id: u64,
    /// Rejects stale asynchronous picker completions for this tab
    #[cfg(target_arch = "wasm32")]
    pub load_generation: u64,
    /// Shared state with background thread (CSV data)
    pub state: Arc<RwLock<crate::state::SharedState>>,
    /// Cancels the indexer for this tab's current load generation
    pub index_cancel_flag: Arc<AtomicBool>,
    /// File path (desktop) or file name (web)
    pub file_path: String,
    /// Display name (filename only)
    pub file_name: String,
    /// Last published headers, cached outside SharedState for nonblocking reload capture
    pub loaded_headers: Vec<String>,
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
    /// Cancels the current filter worker without waiting for it on the UI thread
    pub filter_cancel_flag: Arc<AtomicBool>,
    /// Optimization: Filters that were applied to generate current result
    pub applied_filters: HashMap<usize, FilterCondition>,
    /// Optimization: Sort column used to generate current result
    pub applied_sort_column: Option<usize>,
    /// Optimization: Sort direction used to generate current result
    pub applied_sort_direction: SortDirection,
    /// Duration of last filter operation (for display)
    pub filter_duration: Option<std::time::Duration>,
    /// Whether file_open event has been tracked for this tab
    #[cfg(not(target_arch = "wasm32"))]
    pub file_tracked: bool,
    /// Stable canonical watcher identity for precise unregistering
    #[cfg(not(target_arch = "wasm32"))]
    pub watch_registration: Option<PathBuf>,
    /// Whether this tab failed to register for auto-reload
    #[cfg(not(target_arch = "wasm32"))]
    pub watch_registration_failed: bool,
    /// User intent to restore after a stable external reload finishes
    #[cfg(not(target_arch = "wasm32"))]
    pub external_reload: Option<ExternalReloadState>,
}

impl TabState {
    /// Create a new empty tab
    pub fn new_empty() -> Self {
        Self {
            #[cfg(target_arch = "wasm32")]
            id: next_tab_id(),
            #[cfg(target_arch = "wasm32")]
            load_generation: 0,
            state: Arc::new(RwLock::new(crate::state::SharedState::default())),
            index_cancel_flag: Arc::new(AtomicBool::new(false)),
            file_path: String::new(),
            file_name: String::new(),
            loaded_headers: Vec::new(),
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
            filter_cancel_flag: Arc::new(AtomicBool::new(false)),
            applied_filters: HashMap::new(),
            applied_sort_column: None,
            applied_sort_direction: SortDirection::None,
            filter_duration: None,
            #[cfg(not(target_arch = "wasm32"))]
            file_tracked: false,
            #[cfg(not(target_arch = "wasm32"))]
            watch_registration: None,
            #[cfg(not(target_arch = "wasm32"))]
            watch_registration_failed: false,
            #[cfg(not(target_arch = "wasm32"))]
            external_reload: None,
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
            #[cfg(target_arch = "wasm32")]
            id: next_tab_id(),
            #[cfg(target_arch = "wasm32")]
            load_generation: 0,
            state: Arc::new(RwLock::new(crate::state::SharedState::default())),
            index_cancel_flag: Arc::new(AtomicBool::new(false)),
            file_path,
            file_name,
            loaded_headers: Vec::new(),
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
            filter_cancel_flag: Arc::new(AtomicBool::new(false)),
            applied_filters: HashMap::new(),
            applied_sort_column: None,
            applied_sort_direction: SortDirection::None,
            filter_duration: None,
            #[cfg(not(target_arch = "wasm32"))]
            file_tracked: false,
            #[cfg(not(target_arch = "wasm32"))]
            watch_registration: None,
            #[cfg(not(target_arch = "wasm32"))]
            watch_registration_failed: false,
            #[cfg(not(target_arch = "wasm32"))]
            external_reload: None,
        }
    }

    /// Create a new tab from file name (web)
    #[cfg(target_arch = "wasm32")]
    #[allow(dead_code)] // May be useful for future WASM features
    pub fn from_name(name: String) -> Self {
        Self {
            #[cfg(target_arch = "wasm32")]
            id: next_tab_id(),
            #[cfg(target_arch = "wasm32")]
            load_generation: 0,
            state: Arc::new(RwLock::new(crate::state::SharedState::default())),
            index_cancel_flag: Arc::new(AtomicBool::new(false)),
            file_path: name.clone(),
            file_name: name,
            loaded_headers: Vec::new(),
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
            filter_cancel_flag: Arc::new(AtomicBool::new(false)),
            applied_filters: HashMap::new(),
            applied_sort_column: None,
            applied_sort_direction: SortDirection::None,
            filter_duration: None,
        }
    }

    /// Cancel work for the current file generation without waiting for workers.
    pub fn cancel_workers(&self) {
        self.index_cancel_flag.store(true, Ordering::Release);
        self.sort_state.cancel_flag.store(true, Ordering::Release);
        self.filter_cancel_flag.store(true, Ordering::Release);
        self.search.cancel_flag.store(true, Ordering::Release);
    }

    /// Check if tab is empty (no file loaded)
    #[allow(dead_code)] // May be useful for future features
    pub fn is_empty(&self) -> bool {
        let state = self.state.read();
        matches!(state.load_state, LoadState::Empty)
    }
}
