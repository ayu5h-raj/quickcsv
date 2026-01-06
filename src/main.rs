//! QuickCSV - High-Performance CSV Viewer for macOS and Web
//!
//! A memory-mapped, virtualized CSV viewer that can handle files from 100MB to 2GB+
//! with zero lag. Uses memmap2 for zero-copy file loading and egui for the UI.

// Module declarations
mod csv;
mod state;
mod update;
mod utils;

#[cfg(target_arch = "wasm32")]
use ::csv as csv_crate;
use eframe::egui::{self, Color32, Key};

use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

// Imports from modules
#[cfg(not(target_arch = "wasm32"))]
use csv::init_csv_progressive;
use state::{
    ColumnState, FilterCondition, FilterOperator, FilterState, GoToRowState, JsonViewerState,
    LoadState, RowDetailState, SearchState, SearchStatus, SharedState, SortDirection, SortState,
    MAX_NAV_ROWS,
};
#[cfg(not(target_arch = "wasm32"))]
use update::check_for_updates;
use update::UpdateState;
use utils::{format_file_size, format_json, format_number, looks_like_json, truncate_for_display};

#[cfg(target_arch = "wasm32")]
async fn yield_to_browser() {
    // Use setTimeout(0) to yield to the macro-task queue, allowing UI rendering
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let window = web_sys::window().expect("should have a window");
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// Height of each row in pixels
const ROW_HEIGHT: f32 = 24.0;

/// Highlight color for search matches
const HIGHLIGHT_COLOR: Color32 = Color32::from_rgb(255, 200, 0);

/// Highlight color for current/active search match
const CURRENT_MATCH_COLOR: Color32 = Color32::from_rgb(255, 120, 0);

/// Default column width
const DEFAULT_COLUMN_WIDTH: f32 = 150.0;

/// Main application state
struct FastCsvApp {
    /// Shared state with background thread
    state: Arc<RwLock<SharedState>>,
    /// Vertical scroll offset
    scroll_y: f32,
    /// Horizontal scroll offset
    scroll_x: f32,
    /// Column widths (auto-sized or user-adjusted)
    column_widths: Vec<f32>,
    /// Row cache for rendering (row_index -> parsed fields)
    row_cache: std::collections::HashMap<usize, Vec<String>>,
    /// Last visible row range for cache invalidation
    last_visible_range: (usize, usize),
    /// Search state
    search: SearchState,
    /// JSON viewer popup state
    json_viewer: JsonViewerState,
    /// Dark mode enabled (true = dark, false = light)
    dark_mode: bool,
    /// Column sort state
    sort_state: SortState,
    /// Go to row dialog state
    go_to_row: GoToRowState,
    /// Row detail popup state
    row_detail: RowDetailState,
    /// Update checker state
    update_state: UpdateState,
    /// Column state (visibility, order, undo/redo)
    column_state: ColumnState,
    /// Filter state for column filtering
    filter_state: FilterState,
    /// Cached filtered row indices (None = no filter, Some = indices that pass filter)
    filtered_indices: Option<Vec<usize>>,
    /// Total row count (before filtering) for status bar
    total_row_count: usize,
    /// Filter version (increments when filter changes, used to trigger recompute)
    filter_version: u32,
    /// Last computed filter version (to detect when recompute is needed)
    last_filter_version: u32,
    /// Track previous sorting state to detect completion
    was_sorting: bool,
    /// Channel to receive filtered indices from background thread
    #[allow(clippy::type_complexity)]
    filter_receiver: Option<
        mpsc::Receiver<(
            Vec<usize>,
            std::collections::HashMap<usize, FilterCondition>,
            Option<usize>,
            SortDirection,
            std::time::Duration,
        )>,
    >,
    /// Whether filtering is currently in progress
    is_filtering: Arc<AtomicBool>,
    /// Optimization: Filters that were applied to generate current result
    applied_filters: std::collections::HashMap<usize, FilterCondition>,
    /// Optimization: Sort column used to generate current result
    applied_sort_column: Option<usize>,
    /// Optimization: Sort direction used to generate current result
    applied_sort_direction: SortDirection,
    /// Duration of last filter operation (for display)
    filter_duration: Option<std::time::Duration>,
    /// Channel for receiving loaded file data from async web tasks (WASM)
    #[cfg(target_arch = "wasm32")]
    file_loader_tx: std::sync::mpsc::Sender<(String, Vec<u8>)>,
    /// Channel receiver for file data (WASM)
    #[cfg(target_arch = "wasm32")]
    file_loader_rx: std::sync::mpsc::Receiver<(String, Vec<u8>)>,
}

impl Default for FastCsvApp {
    fn default() -> Self {
        #[cfg(target_arch = "wasm32")]
        let (tx, rx) = std::sync::mpsc::channel();

        Self {
            state: Arc::new(RwLock::new(SharedState::default())),
            scroll_y: 0.0,
            scroll_x: 0.0,
            column_widths: Vec::new(),
            row_cache: std::collections::HashMap::new(),
            last_visible_range: (0, 0),
            search: SearchState::default(),
            json_viewer: JsonViewerState::default(),
            dark_mode: true, // Default to dark mode
            sort_state: SortState::default(),
            go_to_row: GoToRowState::default(),
            row_detail: RowDetailState::default(),
            update_state: UpdateState::default(),
            column_state: ColumnState::default(),
            filter_state: FilterState::default(),
            filtered_indices: None,
            total_row_count: 0,
            filter_version: 0,
            last_filter_version: 0,
            was_sorting: false,
            filter_receiver: None,
            is_filtering: Arc::new(AtomicBool::new(false)),
            applied_filters: std::collections::HashMap::new(),
            applied_sort_column: None,
            applied_sort_direction: SortDirection::None,
            filter_duration: None,
            #[cfg(target_arch = "wasm32")]
            file_loader_tx: tx,
            #[cfg(target_arch = "wasm32")]
            file_loader_rx: rx,
        }
    }
}

impl FastCsvApp {
    /// Open a file dialog and queue the file for loading (WASM)
    #[cfg(target_arch = "wasm32")]
    fn open_file(&mut self, ctx: &egui::Context) {
        let task = rfd::AsyncFileDialog::new()
            .add_filter("CSV", &["csv", "tsv", "txt"])
            .pick_file();

        let tx = self.file_loader_tx.clone();
        let state = self.state.clone();
        let ctx = ctx.clone();

        // Show loading state immediately
        {
            let mut state_guard = state.write();
            state_guard.load_state = LoadState::Indexing;
        }
        ctx.request_repaint();

        wasm_bindgen_futures::spawn_local(async move {
            if let Some(file) = task.await {
                let name = file.file_name();
                let bytes = file.read().await;

                // Send to main thread for processing via channel
                if tx.send((name, bytes)).is_err() {
                    web_sys::console::error_1(&"Failed to send file data".into());
                }

                ctx.request_repaint();
            }
        });
    }

    /// Process a loaded file (WASM) - called from update() when channel receives data
    #[cfg(target_arch = "wasm32")]
    fn load_file_web(&mut self, name: String, bytes: Vec<u8>, ctx: egui::Context) {
        // Reset state
        self.scroll_y = 0.0;
        self.scroll_x = 0.0;
        self.column_widths.clear();
        self.row_cache.clear();
        self.last_visible_range = (0, 0);
        self.sort_state = SortState::default();
        self.column_state = ColumnState::default();
        self.filter_state = FilterState::default();
        self.filtered_indices = None;
        self.applied_filters.clear();
        self.applied_sort_column = None;
        self.search.cancel_flag.store(true, Ordering::SeqCst);
        self.search.current_index = 0;
        self.search.active_query.clear();
        {
            let mut results = self.search.results.write();
            results.navigation_rows.clear();
            results.total_match_count = 0;
            results.status = SearchStatus::Idle;
            results.rows_searched = 0;
            results.nav_limit_reached = false;
        }

        // Set loading state
        {
            let mut state_guard = self.state.write();
            state_guard.csv = None;
            state_guard.load_state = LoadState::Indexing;
            state_guard.error_message = None;
            state_guard.rows_indexed.store(0, Ordering::Relaxed);
            state_guard.cancel_indexing.store(false, Ordering::Relaxed);
            state_guard
                .indexing_complete
                .store(false, Ordering::Relaxed);
        }

        // Use init_csv_web for synchronous parsing
        let state = Arc::clone(&self.state);
        let result = csv::init_csv_web(name, bytes, &state, &ctx);

        if let Err(e) = result {
            let mut state_guard = state.write();
            state_guard.load_state = LoadState::Error;
            state_guard.error_message = Some(e);
            ctx.request_repaint();
        }
    }

    /// Open a file dialog and load the selected CSV file (Native)
    #[cfg(not(target_arch = "wasm32"))]
    fn open_file(&mut self, ctx: &egui::Context) {
        // Cancel any ongoing indexing
        {
            let state = self.state.read();
            state.cancel_indexing.store(true, Ordering::Relaxed);
        }

        // Open native file dialog
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("CSV Files", &["csv", "tsv", "txt"])
            .add_filter("All Files", &["*"])
            .pick_file()
        {
            self.load_file(path, ctx.clone());
        }
    }

    /// Load a CSV file in the background (Native)
    #[cfg(not(target_arch = "wasm32"))]
    fn load_file(&mut self, path: PathBuf, ctx: egui::Context) {
        // Reset state
        self.scroll_y = 0.0;
        self.scroll_x = 0.0;
        self.column_widths.clear();
        self.row_cache.clear();
        self.last_visible_range = (0, 0);
        // Reset sort state
        self.sort_state = SortState::default();
        // Reset column state (visibility, order)
        self.column_state = ColumnState::default();
        // Reset filter state
        self.filter_state = FilterState::default();
        self.filtered_indices = None;
        self.filter_version = 0;
        self.last_filter_version = 0;
        self.filter_receiver = None;
        self.is_filtering.store(false, Ordering::Relaxed);
        self.applied_filters.clear();
        self.applied_sort_column = None;
        self.applied_sort_direction = SortDirection::None;
        self.filter_duration = None;
        // Cancel any ongoing search and clear results
        self.search.cancel_flag.store(true, Ordering::SeqCst);
        self.search.current_index = 0;
        self.search.active_query.clear();
        {
            let mut results = self.search.results.write();
            results.navigation_rows.clear();
            results.total_match_count = 0;
            results.status = SearchStatus::Idle;
            results.rows_searched = 0;
            results.nav_limit_reached = false;
        }

        // Set loading state
        {
            let mut state = self.state.write();
            state.csv = None;
            state.load_state = LoadState::Indexing;
            state.error_message = None;
            state.rows_indexed.store(0, Ordering::Relaxed);
            state.cancel_indexing.store(false, Ordering::Relaxed);
            state.indexing_complete.store(false, Ordering::Relaxed);
        }

        let state = Arc::clone(&self.state);
        let path_clone = path.clone();

        // Start progressive loading - shows data immediately while indexing continues
        thread::spawn(move || {
            let result = init_csv_progressive(&path_clone, &state, &ctx);

            if let Err(e) = result {
                let mut state_guard = state.write();
                state_guard.load_state = LoadState::Error;
                state_guard.error_message = Some(e);
                ctx.request_repaint();
            }
        });
    }

    /// Increment filter version to trigger recompute
    fn mark_filter_changed(&mut self) {
        self.filter_version = self.filter_version.wrapping_add(1);
        // Don't clear indices/cache here to avoid flashing content.
        // They will be updated when async task completes.
    }

    /// Start async filtering task
    /// Start async filtering task
    fn start_async_filtering(&mut self, _ctx: &egui::Context) {
        // If no filters are active, clear filtered indices immediately
        if !self.filter_state.has_active_filters() {
            self.filtered_indices = None;
            self.last_filter_version = self.filter_version;
            self.applied_filters.clear(); // Clear applied state
            return;
        }

        // Check if we are already filtering
        if self.is_filtering.load(Ordering::Relaxed) {
            // If already filtering, maybe we should cancel previous?
            // For now, simple implementation: just start new one, late comer wins visually
            // but cleaner would be cancellation.
        }

        self.is_filtering.store(true, Ordering::Relaxed);
        self.last_filter_version = self.filter_version;

        // Clone state for thread
        let state = self.state.clone();
        let filter_state = self.filter_state.clone();

        // Snapshot current state for optimization check and return
        let current_filters = self.filter_state.filters.clone();
        let sort_col = self.sort_state.column;
        let sort_dir = self.sort_state.direction;

        // Check for optimization:
        let can_optimize = self.filtered_indices.is_some()
            && self.applied_sort_column == sort_col
            && self.applied_sort_direction == sort_dir
            && current_filters.len() > self.applied_filters.len()
            && self
                .applied_filters
                .iter()
                .all(|(k, v)| current_filters.get(k) == Some(v));

        let initial_indices = if can_optimize {
            self.filtered_indices.clone()
        } else {
            None
        };

        // Capture sort state for consistent ordering
        let sorted_indices = {
            let guard = self.sort_state.sorted_indices.read();
            if self.sort_state.direction != SortDirection::None && !guard.is_empty() {
                Some(guard.clone())
            } else {
                None
            }
        };

        // Create channel
        let (sender, receiver) = mpsc::channel();
        self.filter_receiver = Some(receiver);

        let is_filtering = self.is_filtering.clone();

        // Spawn thread
        #[cfg(not(target_arch = "wasm32"))]
        {
            let start_time = std::time::Instant::now();
            thread::spawn(move || {
                let total_rows = {
                    let state_guard = state.read();
                    match &state_guard.csv {
                        Some(csv) => csv.indexed_row_count(),
                        None => {
                            is_filtering.store(false, Ordering::Relaxed);
                            return;
                        }
                    }
                };

                let mut indices = Vec::new();
                {
                    let state_guard = state.read();
                    if let Some(csv) = &state_guard.csv {
                        if let Some(initial) = initial_indices {
                            // OPTIMIZATION: Iterate only over previously filtered rows
                            for &row_idx in initial.iter() {
                                if row_idx < total_rows {
                                    if let Some(fields) = csv.parse_row(row_idx) {
                                        if filter_state.row_matches(&fields) {
                                            indices.push(row_idx);
                                        }
                                    }
                                }
                            }
                        } else if let Some(sorted) = sorted_indices {
                            // Iterate in sorted order
                            for &row_idx in sorted.iter() {
                                if row_idx < total_rows {
                                    if let Some(fields) = csv.parse_row(row_idx) {
                                        if filter_state.row_matches(&fields) {
                                            indices.push(row_idx);
                                        }
                                    }
                                }
                            }
                        } else {
                            // Iterate in natural order
                            for row_idx in 0..total_rows {
                                if let Some(fields) = csv.parse_row(row_idx) {
                                    if filter_state.row_matches(&fields) {
                                        indices.push(row_idx);
                                    }
                                }
                            }
                        }
                    }
                }

                // Send result with metadata
                let duration = start_time.elapsed();
                let _ = sender.send((indices, current_filters, sort_col, sort_dir, duration));
                is_filtering.store(false, Ordering::Relaxed);
            });
        }

        #[cfg(target_arch = "wasm32")]
        {
            // For Web: Async filtering with yields to prevent UI freeze
            // Use web_sys performance.now() for timing
            let start_time_ms = web_sys::window()
                .and_then(|w| w.performance())
                .map(|p| p.now())
                .unwrap_or(0.0);

            let ctx = _ctx.clone();

            wasm_bindgen_futures::spawn_local(async move {
                let total_rows = {
                    let state_guard = state.read();
                    match &state_guard.csv {
                        Some(csv) => csv.indexed_row_count(),
                        None => {
                            is_filtering.store(false, Ordering::Relaxed);
                            return;
                        }
                    }
                };

                let mut indices = Vec::new();
                const BATCH_SIZE: usize = 5000;

                // We need to implement batching loop

                {
                    // Scope for read lock - but we can't hold lock across yield.
                    // So we must re-acquire lock for each batch or row.
                    // THIS IS SLOW.
                    // Better approach: Get CSV ref? No, RwLock doesn't allow long lived ref across await.
                    // We have to iterate and acquire lock.

                    // Since we can't easily iterate an iterator with re-locking,
                    // let's just collect indices to iterate first?

                    // Optimization: if we have initial_indices, that's our list associated.
                    let rows_to_check: Vec<usize> = if let Some(initial) = initial_indices {
                        initial
                    } else if let Some(sorted) = sorted_indices {
                        sorted
                    } else {
                        (0..total_rows).collect()
                    };

                    let mut _processed = 0;
                    for chunk in rows_to_check.chunks(BATCH_SIZE) {
                        let mut batch_indices = Vec::new();
                        {
                            let state_guard = state.read();
                            if let Some(csv) = &state_guard.csv {
                                for &row_idx in chunk {
                                    if row_idx < total_rows {
                                        if let Some(fields) = csv.parse_row(row_idx) {
                                            if filter_state.row_matches(&fields) {
                                                batch_indices.push(row_idx);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        indices.extend(batch_indices);
                        _processed += chunk.len();

                        // Update progress or just yield
                        yield_to_browser().await;
                    }
                }

                // Calculate elapsed time using performance.now()
                let elapsed_ms = web_sys::window()
                    .and_then(|w| w.performance())
                    .map(|p| p.now() - start_time_ms)
                    .unwrap_or(0.0);
                let duration = std::time::Duration::from_secs_f64(elapsed_ms / 1000.0);

                let _ = sender.send((indices, current_filters, sort_col, sort_dir, duration));
                is_filtering.store(false, Ordering::Relaxed);
                ctx.request_repaint();
            });
        }
    }

    /// Sort data by column
    ///
    /// Clicking the same column cycles through: None -> Ascending -> Descending -> None
    /// Clicking a different column starts with Ascending
    ///
    /// Note: Sorting uses the original column index, so it works on the actual data
    /// regardless of whether the column is currently visible or hidden.
    fn sort_by_column(&mut self, col_idx: usize, ctx: &egui::Context) {
        // If already sorting, ignore click
        if self.sort_state.is_sorting.load(Ordering::Relaxed) {
            return;
        }

        // Determine new sort direction
        let new_direction = if self.sort_state.column == Some(col_idx) {
            // Same column - cycle through directions
            match self.sort_state.direction {
                SortDirection::None => SortDirection::Ascending,
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::None,
            }
        } else {
            // Different column - start with ascending
            SortDirection::Ascending
        };

        // If direction is None, clear sorting
        if new_direction == SortDirection::None {
            self.sort_state.cancel_flag.store(true, Ordering::SeqCst);
            self.sort_state = SortState::default();
            self.row_cache.clear();
            return;
        }

        // Cancel any previous sort
        self.sort_state.cancel_flag.store(true, Ordering::SeqCst);

        // Update sort state
        self.sort_state.column = Some(col_idx);
        self.sort_state.direction = new_direction;
        self.sort_state.is_sorting.store(true, Ordering::SeqCst);
        self.sort_state.progress.store(0, Ordering::SeqCst);
        self.sort_state.cancel_flag = Arc::new(AtomicBool::new(false));

        // Clear sorted indices and row cache
        {
            let mut indices = self.sort_state.sorted_indices.write();
            indices.clear();
        }
        self.row_cache.clear();

        // Clone what we need for the background thread
        let state = Arc::clone(&self.state);
        let sorted_indices = Arc::clone(&self.sort_state.sorted_indices);
        let is_sorting = Arc::clone(&self.sort_state.is_sorting);
        let progress = Arc::clone(&self.sort_state.progress);
        let cancel_flag = Arc::clone(&self.sort_state.cancel_flag);
        let ctx = ctx.clone();

        // Spawn background sorting thread
        #[cfg(not(target_arch = "wasm32"))]
        thread::spawn(move || {
            let state_guard = state.read();
            let csv = match &state_guard.csv {
                Some(csv) => csv,
                None => {
                    is_sorting.store(false, Ordering::SeqCst);
                    return;
                }
            };

            let total_rows = csv.indexed_row_count();

            // For very large files, limit sorting for performance
            const MAX_SORT_ROWS: usize = 100_000;
            let rows_to_sort = total_rows.min(MAX_SORT_ROWS);

            // Collect row data for sorting (with progress updates)
            let mut row_data: Vec<(usize, String)> = Vec::with_capacity(rows_to_sort);
            for row_idx in 0..rows_to_sort {
                // Check for cancellation
                if cancel_flag.load(Ordering::Relaxed) {
                    is_sorting.store(false, Ordering::SeqCst);
                    return;
                }

                if let Some(fields) = csv.parse_row(row_idx) {
                    let value = fields.get(col_idx).cloned().unwrap_or_default();
                    row_data.push((row_idx, value));
                }

                // Update progress (collection phase = 0-50%)
                if row_idx % 5000 == 0 {
                    let pct = (row_idx * 50) / rows_to_sort;
                    progress.store(pct, Ordering::Relaxed);
                    ctx.request_repaint();
                }
            }

            progress.store(50, Ordering::Relaxed);
            ctx.request_repaint();

            // Sort by column value
            match new_direction {
                SortDirection::Ascending => {
                    row_data.sort_by(|a, b| {
                        // Try numeric comparison first
                        match (a.1.parse::<f64>(), b.1.parse::<f64>()) {
                            (Ok(a_num), Ok(b_num)) => a_num
                                .partial_cmp(&b_num)
                                .unwrap_or(std::cmp::Ordering::Equal),
                            _ => a.1.to_lowercase().cmp(&b.1.to_lowercase()),
                        }
                    });
                }
                SortDirection::Descending => {
                    row_data.sort_by(|a, b| {
                        // Try numeric comparison first
                        match (a.1.parse::<f64>(), b.1.parse::<f64>()) {
                            (Ok(a_num), Ok(b_num)) => b_num
                                .partial_cmp(&a_num)
                                .unwrap_or(std::cmp::Ordering::Equal),
                            _ => b.1.to_lowercase().cmp(&a.1.to_lowercase()),
                        }
                    });
                }
                SortDirection::None => {}
            }

            progress.store(90, Ordering::Relaxed);
            ctx.request_repaint();

            // Store sorted indices
            {
                let mut indices = sorted_indices.write();
                *indices = row_data.into_iter().map(|(idx, _)| idx).collect();
            }

            progress.store(100, Ordering::Relaxed);
            is_sorting.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        });

        #[cfg(target_arch = "wasm32")]
        {
            let direction = new_direction;
            let is_sorting_clone = Arc::clone(&is_sorting);

            wasm_bindgen_futures::spawn_local(async move {
                // Async collection phase
                const MAX_SORT_ROWS: usize = 100_000;

                let (_total_rows, rows_to_sort) = {
                    let state_guard = state.read();
                    if let Some(csv) = &state_guard.csv {
                        let total = csv.indexed_row_count();
                        (total, total.min(MAX_SORT_ROWS))
                    } else {
                        is_sorting_clone.store(false, Ordering::SeqCst);
                        return;
                    }
                };

                let mut row_data: Vec<(usize, String)> = Vec::with_capacity(rows_to_sort);
                const BATCH_SIZE: usize = 5000;

                for start_idx in (0..rows_to_sort).step_by(BATCH_SIZE) {
                    if cancel_flag.load(Ordering::Relaxed) {
                        is_sorting_clone.store(false, Ordering::SeqCst);
                        return;
                    }

                    // Process batch
                    {
                        let state_guard = state.read();
                        if let Some(csv) = &state_guard.csv {
                            let end_idx = (start_idx + BATCH_SIZE).min(rows_to_sort);
                            for row_idx in start_idx..end_idx {
                                if let Some(fields) = csv.parse_row(row_idx) {
                                    let value = fields.get(col_idx).cloned().unwrap_or_default();
                                    row_data.push((row_idx, value));
                                }
                            }
                        }
                    }

                    // Update progress
                    let pct = (start_idx * 50) / rows_to_sort;
                    progress.store(pct, Ordering::Relaxed);
                    ctx.request_repaint();

                    // Yield
                    yield_to_browser().await;
                }

                progress.store(50, Ordering::Relaxed);

                // Sorting phase (blocking but limited to 100k rows, should be <1s)
                yield_to_browser().await;

                match direction {
                    SortDirection::Ascending => {
                        row_data.sort_by(|a, b| match (a.1.parse::<f64>(), b.1.parse::<f64>()) {
                            (Ok(a_num), Ok(b_num)) => a_num
                                .partial_cmp(&b_num)
                                .unwrap_or(std::cmp::Ordering::Equal),
                            _ => a.1.to_lowercase().cmp(&b.1.to_lowercase()),
                        });
                    }
                    SortDirection::Descending => {
                        row_data.sort_by(|a, b| match (a.1.parse::<f64>(), b.1.parse::<f64>()) {
                            (Ok(a_num), Ok(b_num)) => b_num
                                .partial_cmp(&a_num)
                                .unwrap_or(std::cmp::Ordering::Equal),
                            _ => b.1.to_lowercase().cmp(&a.1.to_lowercase()),
                        });
                    }
                    SortDirection::None => {}
                }

                progress.store(90, Ordering::Relaxed);
                ctx.request_repaint();
                yield_to_browser().await;

                {
                    let mut indices = sorted_indices.write();
                    *indices = row_data.into_iter().map(|(idx, _)| idx).collect();
                }

                progress.store(100, Ordering::Relaxed);
                is_sorting_clone.store(false, Ordering::SeqCst);
                ctx.request_repaint();
            });
        }
    }

    /// Get the actual row index considering sorting
    fn get_actual_row_index(&self, display_index: usize) -> usize {
        if self.sort_state.direction == SortDirection::None {
            return display_index;
        }

        let indices = self.sort_state.sorted_indices.read();
        if indices.is_empty() {
            display_index
        } else if display_index < indices.len() {
            indices[display_index]
        } else {
            // For rows beyond sorted range, return original index
            display_index
        }
    }

    /// Execute search across visible columns only in a background thread
    ///
    /// This search is optimized for large files:
    /// - Counts ALL matches (no limit) for accurate totals
    /// - Only stores limited navigation rows (for prev/next)
    /// - Highlighting is done on-the-fly during rendering
    /// - Only searches visible columns (respects column visibility settings)
    fn execute_search(&mut self, ctx: &egui::Context) {
        let query = self.search.query.trim().to_lowercase();

        // Don't search if query is empty
        if query.is_empty() {
            self.search.cancel_flag.store(true, Ordering::SeqCst);
            let mut results = self.search.results.write();
            results.navigation_rows.clear();
            results.total_match_count = 0;
            results.status = SearchStatus::Idle;
            return;
        }

        // Add to search history (if not duplicate of last entry)
        let query_for_history = self.search.query.trim().to_string();
        if !query_for_history.is_empty() {
            // Remove if already exists (to move to end)
            self.search.history.retain(|h| h != &query_for_history);
            self.search.history.push(query_for_history);
            // Keep history limited to last 50 entries
            if self.search.history.len() > 50 {
                self.search.history.remove(0);
            }
        }
        // Reset history navigation
        self.search.history_index = None;

        // Cancel any previous search
        self.search.cancel_flag.store(true, Ordering::SeqCst);

        // Create new cancel flag for this search
        self.search.cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_flag = Arc::clone(&self.search.cancel_flag);

        // Reset search state
        self.search.current_index = 0;
        self.search.active_query = query.clone();

        // Get total rows and visible columns for progress
        let (total_rows, visible_columns) = {
            let num_columns = {
                let state = self.state.read();
                state.csv.as_ref().map(|c| c.headers.len()).unwrap_or(0)
            };

            // Initialize column order if needed
            if num_columns > 0 {
                self.column_state.init_column_order(num_columns);
            }

            // Get visible columns (original indices)
            let visible = self.column_state.get_visible_columns();

            // Get total rows
            let total = {
                let state = self.state.read();
                state
                    .csv
                    .as_ref()
                    .map(|c| c.indexed_row_count())
                    .unwrap_or(0)
            };

            (total, visible)
        };

        // Initialize results
        {
            let mut results = self.search.results.write();
            results.navigation_rows.clear();
            results.total_match_count = 0;
            results.status = SearchStatus::Searching;
            results.rows_searched = 0;
            results.total_rows = total_rows;
            results.nav_limit_reached = false;
        }

        // Clone what we need for the background thread
        let state = Arc::clone(&self.state);
        let results = Arc::clone(&self.search.results);
        let ctx = ctx.clone();
        let visible_cols = visible_columns; // Clone for thread

        // Spawn background search thread
        #[cfg(not(target_arch = "wasm32"))]
        thread::spawn(move || {
            const BATCH_SIZE: usize = 10000;

            let mut nav_rows: Vec<usize> = Vec::new();
            let mut total_matches: usize = 0;
            let mut nav_limit_reached = false;

            let state_guard = state.read();
            let csv = match &state_guard.csv {
                Some(csv) => csv,
                None => {
                    let mut results = results.write();
                    results.status = SearchStatus::Idle;
                    return;
                }
            };

            for row_idx in 0..csv.indexed_row_count() {
                // Check for cancellation
                if cancel_flag.load(Ordering::Relaxed) {
                    let mut results = results.write();
                    results.status = SearchStatus::Cancelled;
                    ctx.request_repaint();
                    return;
                }

                // Parse and search the row (only in visible columns)
                if let Some(fields) = csv.parse_row(row_idx) {
                    let mut row_has_match = false;
                    // Only search visible columns
                    for &col_idx in &visible_cols {
                        if col_idx < fields.len() {
                            let field = &fields[col_idx];
                            if field.to_lowercase().contains(&query) {
                                total_matches += 1;
                                row_has_match = true;
                            }
                        }
                    }

                    // Store row for navigation (limited)
                    if row_has_match && nav_rows.len() < MAX_NAV_ROWS {
                        nav_rows.push(row_idx);
                    } else if row_has_match && !nav_limit_reached {
                        nav_limit_reached = true;
                    }
                }

                // Update progress every BATCH_SIZE rows
                if (row_idx + 1) % BATCH_SIZE == 0 {
                    let mut results = results.write();
                    results.navigation_rows = nav_rows.clone();
                    results.total_match_count = total_matches;
                    results.rows_searched = row_idx + 1;
                    results.nav_limit_reached = nav_limit_reached;
                    drop(results);
                    ctx.request_repaint();
                }
            }

            // Final update
            let mut results = results.write();
            results.navigation_rows = nav_rows;
            results.total_match_count = total_matches;
            results.rows_searched = csv.indexed_row_count();
            results.nav_limit_reached = nav_limit_reached;
            drop(results);
            ctx.request_repaint();
        });

        #[cfg(target_arch = "wasm32")]
        {
            let state = Arc::clone(&state);
            let results = Arc::clone(&results);
            let ctx = ctx.clone();
            let cancel_flag = Arc::clone(&cancel_flag);

            wasm_bindgen_futures::spawn_local(async move {
                const BATCH_SIZE: usize = 10000;

                let mut nav_rows: Vec<usize> = Vec::new();
                let mut total_matches: usize = 0;
                let mut nav_limit_reached = false;

                let csv_len = {
                    let state_guard = state.read();
                    state_guard
                        .csv
                        .as_ref()
                        .map(|c| c.indexed_row_count())
                        .unwrap_or(0)
                };

                if csv_len == 0 {
                    let mut results = results.write();
                    results.status = SearchStatus::Idle;
                    return;
                }

                // loop through chunks
                for start_idx in (0..csv_len).step_by(BATCH_SIZE) {
                    // Check cancellation
                    if cancel_flag.load(Ordering::Relaxed) {
                        let mut results = results.write();
                        results.status = SearchStatus::Cancelled;
                        ctx.request_repaint();
                        return;
                    }

                    // Process batch
                    let end_idx = (start_idx + BATCH_SIZE).min(csv_len);
                    {
                        let state_guard = state.read();
                        if let Some(csv) = &state_guard.csv {
                            for row_idx in start_idx..end_idx {
                                // Parse and search the row (only in visible columns)
                                if let Some(fields) = csv.parse_row(row_idx) {
                                    let mut row_has_match = false;
                                    // Only search visible columns
                                    for &col_idx in &visible_cols {
                                        if col_idx < fields.len() {
                                            let field = &fields[col_idx];
                                            if field.to_lowercase().contains(&query) {
                                                total_matches += 1;
                                                row_has_match = true;
                                            }
                                        }
                                    }

                                    // Store row for navigation (limited)
                                    if row_has_match && nav_rows.len() < MAX_NAV_ROWS {
                                        nav_rows.push(row_idx);
                                    } else if row_has_match && !nav_limit_reached {
                                        nav_limit_reached = true;
                                    }
                                }
                            }
                        }
                    }

                    // Update UI
                    {
                        let mut results = results.write();
                        results.navigation_rows = nav_rows.clone();
                        results.total_match_count = total_matches;
                        results.rows_searched = end_idx;
                        results.nav_limit_reached = nav_limit_reached;
                        drop(results);
                        ctx.request_repaint();
                    }

                    yield_to_browser().await;
                }

                // Final update
                let mut results = results.write();
                results.navigation_rows = nav_rows;
                results.total_match_count = total_matches;
                results.rows_searched = csv_len;
                results.status = SearchStatus::Complete;
                results.nav_limit_reached = nav_limit_reached;
                drop(results);
                ctx.request_repaint();
            });
        }
    }

    /// Navigate to next matching row
    fn next_match(&mut self) {
        let results = self.search.results.read();
        if results.navigation_rows.is_empty() {
            return;
        }
        self.search.current_index = (self.search.current_index + 1) % results.navigation_rows.len();
        let row = results.navigation_rows[self.search.current_index];
        self.search.scroll_to_row = Some(row);
    }

    /// Navigate to previous matching row
    fn prev_match(&mut self) {
        let results = self.search.results.read();
        if results.navigation_rows.is_empty() {
            return;
        }
        if self.search.current_index == 0 {
            self.search.current_index = results.navigation_rows.len() - 1;
        } else {
            self.search.current_index -= 1;
        }
        let row = results.navigation_rows[self.search.current_index];
        self.search.scroll_to_row = Some(row);
    }

    /// Render the search bar
    fn render_search_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // Read search results state
        let (status, total_matches, nav_row_count, rows_searched, total_rows, nav_limit_reached) = {
            let results = self.search.results.read();
            (
                results.status,
                results.total_match_count,
                results.navigation_rows.len(),
                results.rows_searched,
                results.total_rows,
                results.nav_limit_reached,
            )
        };

        ui.horizontal(|ui| {
            ui.label("🔍");

            // Search input field
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.search.query)
                    .hint_text("Search visible columns...")
                    .desired_width(250.0),
            );

            // Auto-focus when search bar opens
            if self.search.focus_input {
                response.request_focus();
                self.search.focus_input = false;
            }

            // Handle up/down arrow for history navigation (only when focused)
            if response.has_focus() && !self.search.history.is_empty() {
                let up_pressed = ui.input(|i| i.key_pressed(Key::ArrowUp));
                let down_pressed = ui.input(|i| i.key_pressed(Key::ArrowDown));

                if up_pressed {
                    match self.search.history_index {
                        None => {
                            // Save current query and start browsing from most recent
                            self.search.history_temp_query = self.search.query.clone();
                            self.search.history_index = Some(self.search.history.len() - 1);
                            self.search.query =
                                self.search.history.last().cloned().unwrap_or_default();
                        }
                        Some(idx) if idx > 0 => {
                            // Go to older entry
                            self.search.history_index = Some(idx - 1);
                            self.search.query = self.search.history[idx - 1].clone();
                        }
                        _ => {} // Already at oldest entry
                    }
                }

                if down_pressed {
                    match self.search.history_index {
                        Some(idx) if idx < self.search.history.len() - 1 => {
                            // Go to newer entry
                            self.search.history_index = Some(idx + 1);
                            self.search.query = self.search.history[idx + 1].clone();
                        }
                        Some(_) => {
                            // Back to the original query
                            self.search.history_index = None;
                            self.search.query = self.search.history_temp_query.clone();
                        }
                        None => {} // Not browsing history
                    }
                }
            }

            // Execute search on Enter (keep focus on input)
            let enter_pressed = ui.input(|i| i.key_pressed(Key::Enter));
            if enter_pressed && (response.has_focus() || response.lost_focus()) {
                self.execute_search(ctx);
                // Request focus back so user can edit query
                response.request_focus();
            }

            // Search button (disabled during search)
            let is_searching = status == SearchStatus::Searching;
            if ui
                .add_enabled(!is_searching, egui::Button::new("Search"))
                .clicked()
            {
                self.execute_search(ctx);
                // Also keep focus on input after button click
                response.request_focus();
            }

            // Cancel button during search
            if is_searching && ui.button("Cancel").clicked() {
                self.search.cancel_flag.store(true, Ordering::SeqCst);
            }

            ui.separator();

            // Navigation buttons (navigate through rows with matches)
            let has_nav_rows = nav_row_count > 0;

            if ui
                .add_enabled(has_nav_rows && !is_searching, egui::Button::new("◀"))
                .clicked()
            {
                self.prev_match();
            }
            if ui
                .add_enabled(has_nav_rows && !is_searching, egui::Button::new("▶"))
                .clicked()
            {
                self.next_match();
            }

            // Status display
            match status {
                SearchStatus::Searching => {
                    ui.spinner();
                    let progress = if total_rows > 0 {
                        (rows_searched as f32 / total_rows as f32 * 100.0) as usize
                    } else {
                        0
                    };
                    ui.label(format!(
                        "Searching... {}% ({} matches)",
                        progress,
                        format_number(total_matches)
                    ));
                }
                SearchStatus::Complete | SearchStatus::Cancelled => {
                    if total_matches > 0 {
                        // Ensure current_index is valid
                        if self.search.current_index >= nav_row_count && nav_row_count > 0 {
                            self.search.current_index = 0;
                        }
                        // Show total matches and navigation position
                        ui.label(format!("{} matches", format_number(total_matches)));
                        if has_nav_rows {
                            ui.label(format!(
                                "(row {} of {}{})",
                                self.search.current_index + 1,
                                format_number(nav_row_count),
                                if nav_limit_reached { "+" } else { "" }
                            ));
                        }
                        if status == SearchStatus::Cancelled {
                            ui.label("(partial)");
                        }
                    } else if !self.search.active_query.is_empty() {
                        ui.label("No matches");
                    }
                }
                SearchStatus::Idle => {
                    // Show nothing
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(egui_phosphor::regular::X).clicked() {
                    self.search.cancel_flag.store(true, Ordering::SeqCst);
                    self.search.visible = false;
                    self.search.query.clear();
                    self.search.active_query.clear();
                    let mut results = self.search.results.write();
                    results.navigation_rows.clear();
                    results.total_match_count = 0;
                    results.status = SearchStatus::Idle;
                }
            });
        });

        // Request repaint during search to update progress
        if status == SearchStatus::Searching {
            ctx.request_repaint();
        }
    }

    /// Render the virtualized table using egui_extras::TableBuilder
    fn render_table(&mut self, ui: &mut egui::Ui) {
        use egui_extras::{Column, TableBuilder};

        // Extract data from state first, then drop the lock
        let (headers, total_rows, num_columns, _is_indexing) = {
            let state = self.state.read();
            match &state.csv {
                Some(csv) => (
                    csv.headers.clone(),
                    csv.indexed_row_count(),
                    csv.headers.len(),
                    !state.indexing_complete.load(Ordering::Relaxed),
                ),
                None => return,
            }
        };

        // Store total row count for status bar
        self.total_row_count = total_rows;

        // Check if sorting just finished
        let is_sorting = self.sort_state.is_sorting.load(Ordering::Relaxed);
        if self.was_sorting && !is_sorting {
            // Sorting finished, need to recompute filtered indices in sorted order
            self.mark_filter_changed();
        }
        self.was_sorting = is_sorting;

        // Recompute filtered indices if filter changed
        if self.filter_version != self.last_filter_version {
            self.start_async_filtering(ui.ctx());
        }

        // Determine effective row count (filtered or total)
        let display_rows = match &self.filtered_indices {
            Some(indices) => indices.len(),
            None => total_rows,
        };

        // Initialize column state if needed
        self.column_state.init_column_order(num_columns);

        // Get visible columns in display order
        let visible_columns = self.column_state.get_visible_columns();
        let num_visible = visible_columns.len();

        // Ensure we have column widths for visible columns only
        if self.column_widths.len() != num_visible {
            self.column_widths = vec![DEFAULT_COLUMN_WIDTH; num_visible];
        }

        let _text_color = ui.style().visuals.text_color();

        // Handle scroll to row request (from search or go-to-row dialog)
        let scroll_to_row = self
            .search
            .scroll_to_row
            .take()
            .or_else(|| self.go_to_row.scroll_to_row.take());

        // Wrap table in horizontal scroll area for wide tables
        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Build virtualized table
                let mut table = TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .min_scrolled_height(0.0)
                    .sense(egui::Sense::click());

                // Scroll to row if requested (for search navigation)
                if let Some(row) = scroll_to_row {
                    table = table.scroll_to_row(row, Some(egui::Align::Center));
                }

                // Add row number column (fixed width, not resizable)
                table = table.column(Column::exact(60.0).clip(true));

                // Add visible data columns in display order - use clip(true) to prevent overflow
                for col_width in &self.column_widths {
                    table = table.column(Column::initial(*col_width).resizable(true).clip(true));
                }

                // Get search query and current navigation row for highlighting
                // NOTE: We do NOT clone any large data structures - highlighting is done on-the-fly
                let (search_query, current_nav_row) = {
                    let results = self.search.results.read();
                    let nav_row = if !results.navigation_rows.is_empty()
                        && self.search.current_index < results.navigation_rows.len()
                    {
                        Some(results.navigation_rows[self.search.current_index])
                    } else {
                        None
                    };
                    (self.search.active_query.clone(), nav_row)
                };

                // Track which column header was clicked for sorting
                let mut clicked_column: Option<usize> = None;
                // Track which row's detail popup should be opened
                let mut row_to_open_detail: Option<usize> = None;
                // Track drop target for column reordering
                let mut drop_target_idx: Option<usize> = None;

                let current_sort_col = self.sort_state.column;
                let current_sort_dir = self.sort_state.direction;
                let is_sorting = self.sort_state.is_sorting.load(Ordering::Relaxed);
                let sort_progress = self.sort_state.progress.load(Ordering::Relaxed);

                table
                    .header(ROW_HEIGHT, |mut header| {
                        // Row number column header
                            header.col(|ui| {
                            ui.label(egui::RichText::new("#").strong());
                        });

                        // Data column headers (only visible columns in display order)
                        for (display_idx, &original_idx) in visible_columns.iter().enumerate() {
                            if original_idx >= headers.len() {
                                continue;
                            }
                            let col_name = &headers[original_idx];
                            header.col(|ui| {
                                ui.horizontal(|ui| {
                                    // Drag handle using Phosphor icon - always visible
                                    // Allocate actual layout space for the drag handle
                                    let drag_handle_size = egui::vec2(20.0, ROW_HEIGHT);
                                    let (drag_handle_rect, drag_response) = ui.allocate_exact_size(
                                        drag_handle_size,
                                        egui::Sense::drag()
                                    );

                                    // Track drag state
                                    if drag_response.drag_started() {
                                        self.column_state.dragged_column = Some(display_idx);
                                    }

                                    // Check drag state
                                    let is_dragging = self.column_state.dragged_column.is_some();
                                    let is_being_dragged = self.column_state.dragged_column == Some(display_idx);

                                    // Visual feedback - highlight when dragging or hovered
                                    let handle_color = if is_being_dragged {
                                        Color32::from_rgb(100, 180, 255)
                                    } else if drag_response.hovered() {
                                        Color32::from_rgb(180, 180, 180)
                                    } else {
                                        Color32::from_rgb(140, 140, 140)
                                    };

                                    // Add background when hovered/dragging
                                    if drag_response.hovered() || is_being_dragged {
                                        ui.painter().rect_filled(
                                            drag_handle_rect,
                                            2.0,
                                            Color32::from_rgba_unmultiplied(60, 60, 60, 100),
                                        );
                                    }

                                    // Draw the drag handle icon (DOTS_SIX_VERTICAL is more traditional for drag)
                                    ui.painter().text(
                                        drag_handle_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        egui_phosphor::regular::DOTS_SIX_VERTICAL,
                                        egui::TextStyle::Body.resolve(ui.style()),
                                        handle_color,
                                    );

                                    ui.add_space(4.0);

                                    // Header text area (clickable for sorting)
                                    // Note: We get header_rect BEFORE rendering for drop detection
                                    let _header_rect = ui.available_rect_before_wrap();

                                    // Create clickable header with sort indicator
                                    // Note: sorting uses original column index
                                    let sort_indicator = if current_sort_col == Some(original_idx) {
                                        if is_sorting {
                                            format!(" ({sort_progress}%)")
                                        } else {
                                            match current_sort_dir {
                                                SortDirection::Ascending => {
                                                    format!(" {}", egui_phosphor::regular::ARROW_UP)
                                                }
                                                SortDirection::Descending => {
                                                    format!(" {}", egui_phosphor::regular::ARROW_DOWN)
                                                }
                                                SortDirection::None => String::new(),
                                            }
                                        }
                                    } else {
                                        String::new()
                                    };

                                    let header_text = format!("{col_name}{sort_indicator}");

                                    // Dim text if sorting in progress
                                    let text = if is_sorting && current_sort_col == Some(original_idx) {
                                        egui::RichText::new(header_text)
                                            .strong()
                                            .color(Color32::from_rgb(100, 180, 255))
                                    } else {
                                        egui::RichText::new(header_text).strong()
                                    };

                                    // Render the header text as clickable label and get the response
                                    let header_response = ui.add(
                                        egui::Label::new(text).sense(egui::Sense::click())
                                    );

                                    // Get the full header area (handle + actual label rect) for drop detection
                                    let full_header_rect = drag_handle_rect.union(header_response.rect);

                                    // Check if this is a drop target for dragging (use pointer position for better detection)
                                    let pointer_pos = ui.ctx().pointer_hover_pos();
                                    let is_drop_target = is_dragging
                                        && !is_being_dragged
                                        && pointer_pos.is_some_and(|pos| full_header_rect.contains(pos));

                                    // Track this column as potential drop target
                                    if is_drop_target {
                                        drop_target_idx = Some(display_idx);
                                    }

                                    // Visual feedback for drop target
                                    if is_being_dragged {
                                        ui.painter().rect_filled(
                                            full_header_rect,
                                            0.0,
                                            Color32::from_rgba_unmultiplied(100, 180, 255, 100),
                                        );
                                    } else if is_drop_target {
                                        ui.painter().rect_filled(
                                            full_header_rect,
                                            0.0,
                                            Color32::from_rgba_unmultiplied(255, 200, 0, 100),
                                        );
                                    }

                                    // Handle click for sorting (only if not dragging)
                                    // Prevent sorting if user is dragging or just started a drag
                                    if header_response.clicked()
                                        && !is_sorting
                                        && !is_dragging
                                        && self.column_state.dragged_column.is_none()
                                    {
                                        clicked_column = Some(original_idx);
                                    }

                                    // Show tooltip
                                    if is_sorting {
                                        header_response.on_hover_text("Sorting in progress...");
                                    } else {
                                        header_response.on_hover_text("Click to sort");
                                    }

                                    // Filter icon button - shows dropdown on click
                                    let has_filter = self.filter_state.has_filter(original_idx);
                                    let filter_icon = if has_filter {
                                        egui_phosphor::regular::FUNNEL_SIMPLE_X
                                    } else {
                                        egui_phosphor::regular::FUNNEL_SIMPLE
                                    };
                                    let filter_color = if has_filter {
                                        Color32::from_rgb(100, 180, 255)
                                    } else {
                                        Color32::from_rgb(140, 140, 140)
                                    };
                                    let filter_btn = ui.add(
                                        egui::Button::new(
                                            egui::RichText::new(filter_icon).color(filter_color)
                                        )
                                        .frame(false)
                                    );
                                    if filter_btn.clicked() {
                                        if self.filter_state.active_popup == Some(original_idx) {
                                            self.filter_state.close_popup();
                                        } else {
                                            self.filter_state.open_popup(original_idx);
                                        }
                                    }
                                    filter_btn.on_hover_text(if has_filter {
                                        "Click to edit/clear filter"
                                    } else {
                                        "Click to add filter"
                                    });

                                    // Tooltip for drag handle
                                    if drag_response.hovered() {
                                        drag_response.on_hover_text("Drag to reorder column");
                                    }
                                });
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(ROW_HEIGHT, display_rows, |mut row| {
                            let display_idx = row.index();

                            // Determine actual row index (filtered or sorted or original)
                            let actual_row_idx = if let Some(indices) = &self.filtered_indices {
                                *indices.get(display_idx).unwrap_or(&0)
                            } else {
                                self.get_actual_row_index(display_idx)
                            };

                            // Get or parse row data (cache by actual row index)
                            let fields = if let Some(cached) = self.row_cache.get(&actual_row_idx) {
                                cached.clone()
                            } else {
                                // Parse from mmap
                                let state = self.state.read();
                                let fields = if let Some(csv) = &state.csv {
                                    csv.parse_row(actual_row_idx).unwrap_or_default()
                                } else {
                                    vec![]
                                };
                                drop(state);

                                // Cache for next render
                                self.row_cache.insert(actual_row_idx, fields.clone());
                                fields
                            };

                            // Check if this is the current navigation row (for special highlighting)
                            let is_current_nav_row = current_nav_row == Some(actual_row_idx);

                            // Check if this row is highlighted from "Go to Row"
                            let is_goto_highlighted = self.go_to_row.highlight_row == Some(actual_row_idx);

                            // Row number column (1-indexed, shows actual row number)
                            // Double-click to open row detail popup
                            let row_idx_for_detail = actual_row_idx;
                                row.col(|ui| {
                                let rect = ui.available_rect_before_wrap();

                                // Highlight background for "Go to Row" target
                                if is_goto_highlighted {
                                    ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(60, 100, 140));
                                }

                                ui.label(
                                    egui::RichText::new(format_number(actual_row_idx + 1))
                                        .color(if is_goto_highlighted {
                                            Color32::WHITE
                                        } else {
                                            Color32::from_rgb(140, 140, 140)
                                        })
                                        .small(),
                                );

                                // Interact with the full cell area for easier clicking
                                let response = ui.interact(rect, ui.id().with("row_click"), egui::Sense::click());
                                if response.double_clicked() {
                                    row_to_open_detail = Some(row_idx_for_detail);
                                }
                                response.on_hover_text("Double-click to view row details");
                            });

                            // Render each visible cell with ON-THE-FLY search highlighting
                            // Iterate over visible columns in display order
                            for &original_idx in visible_columns.iter() {
                                if original_idx >= fields.len() {
                                    // Column doesn't exist in this row, render empty cell
                                    row.col(|ui| {
                                        ui.label("");
                                    });
                                    continue;
                                }

                                let field = &fields[original_idx];
                                row.col(|ui| {
                                    // Check if this cell matches the search query
                                    // Optimization: skip expensive to_lowercase on huge fields
                                    let is_match = !search_query.is_empty()
                                        && field.len() < 100_000  // Skip search highlight on >100KB fields
                                        && field.to_lowercase().contains(&search_query);

                                    // Check if this looks like JSON (optimized for large strings)
                                    let is_json = looks_like_json(field);

                                    // PERFORMANCE: Truncate display string for huge fields
                                    // Full content is still available via double-click popup
                                    let display_text = truncate_for_display(field);

                                    // Create a clickable label
                                    // Priority: search match > go-to-row highlight > JSON styling > default
                                    let response = if is_match && is_current_nav_row {
                                        // Current navigation row match - bright orange
                                        let rect = ui.available_rect_before_wrap();
                                        ui.painter().rect_filled(rect, 2.0, CURRENT_MATCH_COLOR);
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(display_text.as_ref()).color(Color32::BLACK),
                                            )
                                                .sense(egui::Sense::click()),
                                        )
                                    } else if is_match {
                                        // Other matches - yellow background
                                        let rect = ui.available_rect_before_wrap();
                                        ui.painter().rect_filled(rect, 2.0, HIGHLIGHT_COLOR);
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(display_text.as_ref()).color(Color32::BLACK),
                                            )
                                                .sense(egui::Sense::click()),
                                        )
                                    } else if is_goto_highlighted {
                                        // Go-to-row highlight - blue background
                                        let rect = ui.available_rect_before_wrap();
                                        ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(60, 100, 140));
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(display_text.as_ref()).color(Color32::WHITE),
                                            )
                                                .sense(egui::Sense::click()),
                                        )
                                    } else if is_json {
                                        // JSON content - show with subtle indicator
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(display_text.as_ref())
                                                    .color(Color32::from_rgb(100, 180, 255)),
                                            )
                                                .sense(egui::Sense::click()),
                                        )
                                    } else {
                                        ui.add(egui::Label::new(display_text.as_ref()).sense(egui::Sense::click()))
                                    };

                                    // Handle click to clear go-to-row highlight
                                    if response.clicked() {
                                        self.go_to_row.highlight_row = None;
                                    }

                                    // Handle double-click to open cell viewer (works for any cell)
                                    if response.double_clicked() {
                                        // Get column name (use original index)
                                        let col_name = headers
                                            .get(original_idx)
                                            .cloned()
                                            .unwrap_or_else(|| format!("Column {}", original_idx + 1));

                                        // Try to format as JSON if it looks like JSON
                                        let formatted =
                                            if is_json { format_json(field) } else { None };

                                        self.json_viewer.open = true;
                                        self.json_viewer.raw_content = field.clone();
                                        self.json_viewer.formatted_content =
                                            formatted.clone().unwrap_or_else(|| field.clone());
                                        self.json_viewer.is_valid_json =
                                            is_json && formatted.is_some();
                                        self.json_viewer.row = actual_row_idx;
                                        self.json_viewer.col = original_idx; // Store original index
                                        self.json_viewer.column_name = col_name;
                                    }

                                    // Show tooltip for truncated cells
                                    // Limit tooltip size to avoid performance issues with huge fields
                                    // Full content available via double-click popup
                                    if field.len() > 50 {
                                        if field.len() <= 500 {
                                            response.on_hover_text(field);
                                        } else {
                                            // For very large fields, show truncated preview + hint
                                            let preview: String = field.chars().take(500).collect();
                                            response.on_hover_text(format!("{}…\n\n(Double-click to view full content: {} bytes)", preview, field.len()));
                                        }
                                    }
                                });
                            }

                            // Note: We don't need to fill remaining columns anymore
                            // since we only render visible columns
                        });
                    });

                // Handle column header click for sorting (after table is built)
                if let Some(col_idx) = clicked_column {
                    self.sort_by_column(col_idx, ui.ctx());
                }

                // Handle column drop for reordering
                // Check if pointer was released while we have a dragged column
                let pointer_released = ui.input(|i| i.pointer.any_released());
                if pointer_released {
                    if let Some(dragged) = self.column_state.dragged_column {
                        if let Some(target) = drop_target_idx {
                            if dragged != target {
                                self.column_state.reorder_visible_columns(dragged, target);
                            }
                        }
                        self.column_state.dragged_column = None;
                    }
                }

                // Handle row number double-click for row detail popup
                if let Some(row_idx) = row_to_open_detail {
                    self.open_row_detail(row_idx);
                }
            }); // End of horizontal scroll area

        // Prune old cache entries (keep last 2000 rows in cache)
        if self.row_cache.len() > 2000 {
            let keys: Vec<usize> = self.row_cache.keys().cloned().collect();
            for key in keys.into_iter().take(self.row_cache.len() - 1000) {
                self.row_cache.remove(&key);
            }
        }
    }

    /// Render the status bar
    fn render_status_bar(&self, ui: &mut egui::Ui) {
        let state = self.state.read();

        ui.horizontal(|ui| match state.load_state {
            LoadState::Empty => {
                ui.label("No file loaded");
            }
            LoadState::Indexing => {
                let rows = state.rows_indexed.load(Ordering::Relaxed);
                ui.spinner();
                ui.label(format!("Indexing... {} rows", format_number(rows)));
            }
            LoadState::Ready => {
                if let Some(csv) = &state.csv {
                    let row_count = csv.indexed_row_count();
                    let is_still_indexing = !state.indexing_complete.load(Ordering::Relaxed);
                    let is_filtering = self.is_filtering.load(Ordering::Relaxed);

                    if is_still_indexing {
                        ui.spinner();
                        ui.label(format!("Rows: {}...", format_number(row_count)));
                    } else if is_filtering {
                        ui.spinner();
                        ui.label(format!("Filtering... (Rows: {})", format_number(row_count)));
                    } else if let Some(filtered) = &self.filtered_indices {
                        let duration_text = if let Some(d) = self.filter_duration {
                            format!(" • {:.2}s", d.as_secs_f64())
                        } else {
                            String::new()
                        };
                        ui.label(format!(
                            "Rows: {} (of {}){}",
                            format_number(filtered.len()),
                            format_number(row_count),
                            duration_text
                        ))
                        .on_hover_text("Number of rows matching active filters");
                    } else {
                        ui.label(format!("Rows: {}", format_number(row_count)));
                    }
                    ui.separator();
                    ui.label(format!("Columns: {}", csv.headers.len()));
                    ui.separator();
                    ui.label(format!("Delimiter: {}", csv.delimiter_name()));
                    ui.separator();
                    ui.label(format!("Size: {}", format_file_size(csv.file_size)));
                    ui.separator();
                    ui.label(csv.path.file_name().unwrap_or_default().to_string_lossy());
                }
            }
            LoadState::Error => {
                ui.colored_label(
                    egui::Color32::RED,
                    state.error_message.as_deref().unwrap_or("Unknown error"),
                );
            }
        });
    }

    /// Render the JSON viewer popup
    fn render_json_popup(&mut self, ctx: &egui::Context) {
        if !self.json_viewer.open {
            return;
        }

        // Handle Escape to close
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.json_viewer.open = false;
            return;
        }

        // Copy values to avoid borrow issues
        let column_name = self.json_viewer.column_name.clone();
        let row_num = format_number(self.json_viewer.row + 1);
        let is_valid = self.json_viewer.is_valid_json;
        let formatted = self.json_viewer.formatted_content.clone();
        let raw = self.json_viewer.raw_content.clone();

        let mut should_close = false;

        // Determine title based on content type
        let title = if is_valid {
            "📄 Cell Viewer (JSON)"
        } else {
            "📄 Cell Viewer"
        };

        egui::Window::new(title)
            .default_size([550.0, 450.0])
            .min_size([400.0, 300.0])
            .resizable(true)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(4.0);

                // Header info bar with location and content type
                ui.horizontal(|ui| {
                    // Location info with icons
                    ui.label(egui::RichText::new("📍").size(14.0));
                    ui.label(
                        egui::RichText::new(format!("Row {row_num}"))
                            .color(Color32::from_rgb(180, 180, 180)),
                    );
                    ui.label(egui::RichText::new("•").color(Color32::from_rgb(100, 100, 100)));
                    ui.label(
                        egui::RichText::new(&column_name)
                            .color(Color32::from_rgb(130, 180, 230))
                            .strong(),
                    );

                    // Right-aligned content type badge (only show for JSON)
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if is_valid {
                            egui::Frame::NONE
                                .fill(Color32::from_rgb(30, 70, 40))
                                .corner_radius(egui::CornerRadius::same(4))
                                .inner_margin(egui::Margin::symmetric(8, 4))
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new("JSON")
                                            .color(Color32::from_rgb(120, 220, 120))
                                            .size(12.0),
                                    );
                                });
                        } else {
                            // Show character count for plain text
                            ui.label(
                                egui::RichText::new(format!("{} chars", raw.len()))
                                    .color(Color32::from_rgb(150, 150, 150))
                                    .size(12.0),
                            );
                        }
                    });
                });

                ui.add_space(8.0);

                // JSON content area with distinct background
                egui::Frame::NONE
                    .fill(Color32::from_rgb(25, 25, 30))
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(12.0)
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(50, 50, 60)))
                    .show(ui, |ui| {
                        egui::ScrollArea::both()
                            .auto_shrink([false, false])
                            .max_height(300.0)
                            .show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut formatted.as_str())
                                        .font(egui::TextStyle::Monospace)
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(15)
                                        .interactive(false)
                                        .text_color(Color32::from_rgb(200, 200, 210)),
                                );
                            });
                    });

                ui.add_space(12.0);

                // Action buttons with better styling
                ui.horizontal(|ui| {
                    // Copy buttons on the left
                    if ui
                        .add(
                            egui::Button::new(format!(
                                "{} Copy Formatted",
                                egui_phosphor::regular::COPY
                            ))
                            .min_size([140.0, 28.0].into()),
                        )
                        .clicked()
                    {
                        #[cfg(not(target_arch = "wasm32"))]
                        if let Ok(mut clipboard) = arboard::Clipboard::new() {
                            let _ = clipboard.set_text(&formatted);
                        }
                    }

                    ui.add_space(8.0);

                    if ui
                        .add(
                            egui::Button::new(format!(
                                "{} Copy Raw",
                                egui_phosphor::regular::FILE_TEXT
                            ))
                            .min_size([120.0, 28.0].into()),
                        )
                        .clicked()
                    {
                        #[cfg(not(target_arch = "wasm32"))]
                        if let Ok(mut clipboard) = arboard::Clipboard::new() {
                            let _ = clipboard.set_text(&raw);
                        }
                    }

                    // Close button on the right
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(format!("{} Close", egui_phosphor::regular::X))
                                    .min_size([80.0, 28.0].into())
                                    .fill(Color32::from_rgb(60, 60, 70)),
                            )
                            .clicked()
                        {
                            should_close = true;
                        }
                    });
                });

                ui.add_space(4.0);
            });

        if should_close {
            self.json_viewer.open = false;
        }
    }

    /// Render the Go to Row dialog
    fn render_go_to_row_dialog(&mut self, ctx: &egui::Context) {
        if !self.go_to_row.open {
            return;
        }

        // Get total rows for validation
        let total_rows = {
            let state = self.state.read();
            state
                .csv
                .as_ref()
                .map(|c| c.indexed_row_count())
                .unwrap_or(0)
        };

        if total_rows == 0 {
            self.go_to_row.open = false;
            return;
        }

        let mut should_close = false;
        let mut should_go = false;

        egui::Window::new("Go to Row")
            .default_size([300.0, 100.0])
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.label("Row number:");
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.go_to_row.input)
                            .hint_text(format!("1 - {}", format_number(total_rows)))
                            .desired_width(150.0),
                    );

                    // Auto-focus on open
                    if self.go_to_row.focus_input {
                        response.request_focus();
                        self.go_to_row.focus_input = false;
                    }

                    // Handle Enter key
                    if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        should_go = true;
                    }
                });

                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    if ui.button("Go").clicked() {
                        should_go = true;
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        should_close = true;
                    }
                });
            });

        if should_go {
            // Parse and validate row number
            if let Ok(row_num) = self
                .go_to_row
                .input
                .trim()
                .replace(',', "")
                .parse::<usize>()
            {
                if row_num >= 1 && row_num <= total_rows {
                    // Convert to 0-indexed
                    let row_idx = row_num - 1;
                    self.go_to_row.scroll_to_row = Some(row_idx);
                    self.go_to_row.highlight_row = Some(row_idx);
                    should_close = true;
                }
            }
        }

        if should_close {
            self.go_to_row.open = false;
        }
    }

    /// Open row detail popup for a specific row
    fn open_row_detail(&mut self, row_index: usize) {
        // Get the row data and headers
        let (fields, headers) = {
            let state = self.state.read();
            if let Some(csv) = &state.csv {
                let fields = csv.parse_row(row_index).unwrap_or_default();
                let headers = csv.headers.clone();
                (fields, headers)
            } else {
                return;
            }
        };

        self.row_detail.open = true;
        self.row_detail.row_index = row_index;
        self.row_detail.fields = fields;
        self.row_detail.headers = headers;
        self.row_detail.expanded_fields.clear();
    }

    /// Render the row detail popup
    fn render_row_detail_popup(&mut self, ctx: &egui::Context) {
        if !self.row_detail.open {
            return;
        }

        // Handle Escape to close
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.row_detail.open = false;
            return;
        }

        let row_num = format_number(self.row_detail.row_index + 1);
        let num_fields = self.row_detail.fields.len();

        let mut should_close = false;
        let mut open_json_for: Option<(usize, String, String)> = None;
        let mut toggle_expand: Option<usize> = None;

        egui::Window::new(format!("📋 Row {} Details", row_num))
            .default_size([600.0, 500.0])
            .min_size([400.0, 300.0])
            .resizable(true)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(4.0);

                // Header info bar
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{} columns", num_fields))
                            .color(Color32::from_rgb(150, 150, 150)),
                    );

                    // Copy buttons on the right
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(format!("{} Copy as CSV", egui_phosphor::regular::COPY))
                            .clicked()
                        {
                            #[cfg(not(target_arch = "wasm32"))]
                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                let csv_row = self
                                    .row_detail
                                    .fields
                                    .iter()
                                    .map(|f| {
                                        if f.contains(',') || f.contains('"') || f.contains('\n') {
                                            format!("\"{}\"", f.replace('"', "\"\""))
                                        } else {
                                            f.clone()
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join(",");
                                let _ = clipboard.set_text(&csv_row);
                            }
                        }

                        if ui
                            .button(format!("{} Copy as JSON", egui_phosphor::regular::CODE))
                            .clicked()
                        {
                            #[cfg(not(target_arch = "wasm32"))]
                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                let mut json_obj = serde_json::Map::new();
                                for (i, field) in self.row_detail.fields.iter().enumerate() {
                                    let header = self
                                        .row_detail
                                        .headers
                                        .get(i)
                                        .cloned()
                                        .unwrap_or_else(|| format!("col_{}", i));
                                    json_obj
                                        .insert(header, serde_json::Value::String(field.clone()));
                                }
                                if let Ok(json_str) = serde_json::to_string_pretty(&json_obj) {
                                    let _ = clipboard.set_text(&json_str);
                                }
                            }
                        }
                    });
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                // Scrollable field list
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(380.0)
                    .show(ui, |ui| {
                        for (i, field) in self.row_detail.fields.iter().enumerate() {
                            let header = self
                                .row_detail
                                .headers
                                .get(i)
                                .cloned()
                                .unwrap_or_else(|| format!("Column {}", i + 1));
                            let is_expanded = self.row_detail.expanded_fields.contains(&i);
                            let is_large = field.len() > 200;
                            let is_json = looks_like_json(field);

                            egui::Frame::NONE
                                .fill(Color32::from_rgb(30, 30, 35))
                                .corner_radius(egui::CornerRadius::same(4))
                                .inner_margin(8.0)
                                .outer_margin(egui::Margin::symmetric(0, 2))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        // Column name
                                        ui.label(
                                            egui::RichText::new(&header)
                                                .color(Color32::from_rgb(130, 180, 230))
                                                .strong(),
                                        );

                                        // Show size for large values
                                        if is_large {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "({} chars)",
                                                    field.len()
                                                ))
                                                .color(Color32::from_rgb(100, 100, 100))
                                                .size(11.0),
                                            );
                                        }

                                        // Action buttons on the right
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                // Copy button
                                                if ui
                                                    .small_button(egui_phosphor::regular::COPY)
                                                    .on_hover_text("Copy value")
                                                    .clicked()
                                                {
                                                    #[cfg(not(target_arch = "wasm32"))]
                                                    if let Ok(mut clipboard) =
                                                        arboard::Clipboard::new()
                                                    {
                                                        let _ = clipboard.set_text(field);
                                                    }
                                                }

                                                // JSON button if it looks like JSON
                                                if is_json
                                                    && ui
                                                        .small_button(egui_phosphor::regular::CODE)
                                                        .on_hover_text("View as JSON")
                                                        .clicked()
                                                {
                                                    open_json_for = Some((
                                                        self.row_detail.row_index,
                                                        header.clone(),
                                                        field.clone(),
                                                    ));
                                                }

                                                // Expand/collapse for large values
                                                if is_large {
                                                    let btn_text = if is_expanded {
                                                        egui_phosphor::regular::CARET_UP
                                                    } else {
                                                        egui_phosphor::regular::CARET_DOWN
                                                    };
                                                    if ui
                                                        .small_button(btn_text)
                                                        .on_hover_text(if is_expanded {
                                                            "Collapse"
                                                        } else {
                                                            "Expand"
                                                        })
                                                        .clicked()
                                                    {
                                                        toggle_expand = Some(i);
                                                    }
                                                }
                                            },
                                        );
                                    });

                                    ui.add_space(4.0);

                                    // Value display
                                    let display_value = if is_large && !is_expanded {
                                        format!("{}...", &field[..field.len().min(200)])
                                    } else {
                                        field.clone()
                                    };

                                    ui.add(
                                        egui::TextEdit::multiline(&mut display_value.as_str())
                                            .font(egui::TextStyle::Monospace)
                                            .desired_width(f32::INFINITY)
                                            .desired_rows(if is_large && is_expanded {
                                                8
                                            } else {
                                                1
                                            })
                                            .interactive(false)
                                            .text_color(Color32::from_rgb(200, 200, 210)),
                                    );
                                });
                        }
                    });

                ui.add_space(8.0);

                // Close button
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(format!("{} Close", egui_phosphor::regular::X))
                                    .min_size([80.0, 28.0].into())
                                    .fill(Color32::from_rgb(60, 60, 70)),
                            )
                            .clicked()
                        {
                            should_close = true;
                        }
                    });
                });
            });

        // Handle toggle expand
        if let Some(idx) = toggle_expand {
            if self.row_detail.expanded_fields.contains(&idx) {
                self.row_detail.expanded_fields.remove(&idx);
            } else {
                self.row_detail.expanded_fields.insert(idx);
            }
        }

        // Handle opening JSON viewer
        if let Some((row, col_name, value)) = open_json_for {
            self.row_detail.open = false;
            self.json_viewer.row = row;
            self.json_viewer.col = 0;
            self.json_viewer.column_name = col_name;
            self.json_viewer.raw_content = value.clone();
            if let Some(formatted) = format_json(&value) {
                self.json_viewer.formatted_content = formatted;
                self.json_viewer.is_valid_json = true;
            } else {
                self.json_viewer.formatted_content = value;
                self.json_viewer.is_valid_json = false;
            }
            self.json_viewer.open = true;
        }

        if should_close {
            self.row_detail.open = false;
        }
    }

    /// Render the filter popup for a column
    fn render_filter_popup(&mut self, ctx: &egui::Context) {
        let Some(col_idx) = self.filter_state.active_popup else {
            return;
        };

        // Get column name for title
        let col_name = {
            let state = self.state.read();
            state
                .csv
                .as_ref()
                .and_then(|csv| csv.headers.get(col_idx).cloned())
                .unwrap_or_else(|| format!("Column {}", col_idx + 1))
        };

        // Handle Escape to close
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.filter_state.close_popup();
            return;
        }

        let mut should_close = false;
        let mut should_apply = false;
        let mut should_clear = false;

        egui::Window::new(format!("Filter: {}", col_name))
            .resizable(false)
            .collapsible(false)
            .default_width(250.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(4.0);

                // Operator dropdown
                ui.horizontal(|ui| {
                    ui.label("Operator:");
                    egui::ComboBox::from_id_salt("filter_operator")
                        .selected_text(self.filter_state.selected_operator.display_name())
                        .show_ui(ui, |ui| {
                            for op in FilterOperator::all() {
                                ui.selectable_value(
                                    &mut self.filter_state.selected_operator,
                                    *op,
                                    op.display_name(),
                                );
                            }
                        });
                });

                ui.add_space(4.0);

                // Value input (not needed for Empty/NotEmpty)
                let needs_value = !matches!(
                    self.filter_state.selected_operator,
                    FilterOperator::Empty | FilterOperator::NotEmpty
                );
                if needs_value {
                    ui.horizontal(|ui| {
                        ui.label("Value:");
                        let response = ui.text_edit_singleline(&mut self.filter_state.filter_input);
                        // Apply on Enter
                        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            should_apply = true;
                        }
                    });
                }

                ui.add_space(8.0);

                // Buttons
                ui.horizontal(|ui| {
                    if ui.button("Apply").clicked() {
                        should_apply = true;
                    }
                    if self.filter_state.has_filter(col_idx) && ui.button("Clear").clicked() {
                        should_clear = true;
                    }
                    if ui.button("Cancel").clicked() {
                        should_close = true;
                    }
                });
            });

        // Handle actions after UI
        if should_apply {
            let operator = self.filter_state.selected_operator;
            let value = self.filter_state.filter_input.clone();
            self.filter_state.apply_filter(col_idx, operator, value);
            self.mark_filter_changed();
        } else if should_clear {
            self.filter_state.clear_filter(col_idx);
            self.filter_state.close_popup();
            self.mark_filter_changed();
        } else if should_close {
            self.filter_state.close_popup();
        }
    }

    /// Render the Column Manager dialog
    fn render_column_manager(&mut self, ctx: &egui::Context) {
        if !self.column_state.manager_open {
            return;
        }

        // Handle Escape to close
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.column_state.manager_open = false;
            return;
        }

        // Get headers from state
        let headers = {
            let state = self.state.read();
            state
                .csv
                .as_ref()
                .map(|c| c.headers.clone())
                .unwrap_or_default()
        };

        if headers.is_empty() {
            self.column_state.manager_open = false;
            return;
        }

        // Initialize column order if needed
        self.column_state.init_column_order(headers.len());

        let mut should_close = false;
        let mut move_up: Option<usize> = None;
        let mut move_down: Option<usize> = None;

        egui::Window::new("Column Manager")
            .default_size([400.0, 500.0])
            .resizable(true)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(8.0);

                // Action buttons at top
                ui.horizontal(|ui| {
                    if ui.button("Show All").clicked() {
                        self.column_state.show_all_columns();
                    }
                    if ui.button("Hide All").clicked() {
                        self.column_state.hide_all_columns();
                    }
                    if ui.button("Reset Order").clicked() {
                        self.column_state.reset_column_order();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let undo_enabled = !self.column_state.undo_stack.is_empty();
                        let redo_enabled = !self.column_state.redo_stack.is_empty();

                        if ui
                            .add_enabled(redo_enabled, egui::Button::new("Redo"))
                            .clicked()
                        {
                            self.column_state.redo();
                        }
                        if ui
                            .add_enabled(undo_enabled, egui::Button::new("Undo"))
                            .clicked()
                        {
                            self.column_state.undo();
                        }
                    });
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                // Column list
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(350.0)
                    .show(ui, |ui| {
                        // Show columns in their current display order
                        let column_order = self.column_state.column_order.clone();
                        let visible_columns = self.column_state.get_visible_columns();

                        // Build a map: original_idx -> visible_index
                        let original_to_visible: std::collections::HashMap<usize, usize> = visible_columns
                            .iter()
                            .enumerate()
                            .map(|(vis_idx, &orig_idx)| (orig_idx, vis_idx))
                            .collect();

                        for &original_idx in column_order.iter() {
                            if original_idx >= headers.len() {
                                continue;
                            }

                            let header = &headers[original_idx];
                            let is_hidden = self.column_state.hidden_columns.contains(&original_idx);
                            let visible_idx = original_to_visible.get(&original_idx);

                            ui.horizontal(|ui| {
                                // Visibility checkbox
                                let mut visible = !is_hidden;
                                if ui.checkbox(&mut visible, "").changed() {
                                    self.column_state.toggle_column(original_idx);
                                }

                                // Column name
                                let mut text = egui::RichText::new(header);
                                if is_hidden {
                                    text = text.strikethrough().color(Color32::GRAY);
                                } else {
                                    text = text.color(Color32::WHITE);
                                }
                                ui.label(text);

                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    // Move buttons - only enabled for visible columns
                                    // Reordering works on visible columns only
                                    if let Some(&vis_idx) = visible_idx {
                                        let can_move_up = vis_idx > 0;
                                        let can_move_down = vis_idx < visible_columns.len() - 1;

                                        if ui
                                            .add_enabled(
                                                can_move_down,
                                                egui::Button::new(egui_phosphor::regular::ARROW_DOWN)
                                                    .small(),
                                            )
                                            .clicked()
                                        {
                                            move_down = Some(vis_idx);
                                        }
                                        if ui
                                            .add_enabled(
                                                can_move_up,
                                                egui::Button::new(egui_phosphor::regular::ARROW_UP)
                                                    .small(),
                                            )
                                            .clicked()
                                        {
                                            move_up = Some(vis_idx);
                                        }
                                    } else {
                                        // Hidden columns - disable move buttons
                                        ui.add_enabled(false, egui::Button::new(egui_phosphor::regular::ARROW_DOWN).small());
                                        ui.add_enabled(false, egui::Button::new(egui_phosphor::regular::ARROW_UP).small());
                                    }
                                });
                            });
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                // Info text
                ui.label(
                    egui::RichText::new("Tip: Use checkboxes to show/hide columns. Use arrows to reorder visible columns.")
                        .small()
                        .color(Color32::GRAY),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "{} of {} columns visible",
                        self.column_state.get_visible_columns().len(),
                        headers.len()
                    ))
                    .small()
                    .color(Color32::GRAY),
                );

                ui.add_space(8.0);

                // Close button
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(format!("{} Close", egui_phosphor::regular::X))
                                    .min_size([80.0, 28.0].into())
                                    .fill(Color32::from_rgb(60, 60, 70)),
                            )
                            .clicked()
                        {
                            should_close = true;
                        }
                    });
                });
            });

        // Handle column moves (work on visible columns only)
        if let Some(idx) = move_up {
            if idx > 0 {
                self.column_state.reorder_visible_columns(idx, idx - 1);
            }
        }
        if let Some(idx) = move_down {
            let visible_count = self.column_state.get_visible_columns().len();
            if idx < visible_count - 1 {
                self.column_state.reorder_visible_columns(idx, idx + 1);
            }
        }

        if should_close {
            self.column_state.manager_open = false;
        }
    }
}

impl eframe::App for FastCsvApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll file loader channel (WASM only)
        #[cfg(target_arch = "wasm32")]
        {
            while let Ok((name, bytes)) = self.file_loader_rx.try_recv() {
                self.load_file_web(name, bytes, ctx.clone());
            }
        }

        // Check for updates on startup (only once)
        if !self.update_state.check_initiated {
            self.update_state.check_initiated = true;
            #[cfg(not(target_arch = "wasm32"))]
            check_for_updates(
                Arc::clone(&self.update_state.latest_version),
                Arc::clone(&self.update_state.update_available),
                ctx.clone(),
            );
        }

        // Check for async filtering results
        if let Some(receiver) = &self.filter_receiver {
            match receiver.try_recv() {
                Ok((indices, filters, sort_col, sort_dir, duration)) => {
                    self.filtered_indices = Some(indices);
                    self.applied_filters = filters;
                    self.applied_sort_column = sort_col;
                    self.applied_sort_direction = sort_dir;
                    self.filter_duration = Some(duration);
                    self.filter_receiver = None; // Done receiving
                    self.is_filtering.store(false, Ordering::Relaxed);
                    self.row_cache.clear(); // Clear cache for new view
                    ctx.request_repaint(); // Trigger update to show results
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // Still waiting - keep spinner spinning
                    ctx.request_repaint();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    // Thread panicked or disconnected
                    self.filter_receiver = None;
                    self.is_filtering.store(false, Ordering::Relaxed);
                    ctx.request_repaint();
                }
            }
        }

        // Handle keyboard shortcuts
        ctx.input(|i| {
            // Cmd/Ctrl+F to toggle search
            if i.modifiers.command && i.key_pressed(Key::F) {
                self.search.visible = !self.search.visible;
                if self.search.visible {
                    // Request focus on the search input and select all text
                    self.search.focus_input = true;
                }
                // Note: We don't clear the query when closing - it persists for convenience
            }
            // Escape to close search (keeps query text for convenience)
            if i.key_pressed(Key::Escape) && self.search.visible {
                self.search.cancel_flag.store(true, Ordering::SeqCst);
                self.search.visible = false;
                // Reset history navigation
                self.search.history_index = None;
            }
            // F3 or Cmd+G for next match
            if i.key_pressed(Key::F3) || (i.modifiers.command && i.key_pressed(Key::G)) {
                if i.modifiers.shift {
                    self.prev_match();
                } else {
                    self.next_match();
                }
            }
            // Cmd+L to open Go to Row dialog
            if i.modifiers.command && i.key_pressed(Key::L) {
                self.go_to_row.open = true;
                self.go_to_row.focus_input = true;
                self.go_to_row.input.clear();
            }
            // Cmd+Shift+C to open Column Manager
            if i.modifiers.command && i.modifiers.shift && i.key_pressed(Key::C) {
                self.column_state.manager_open = true;
            }
            // Cmd+Z for undo column actions
            if i.modifiers.command && i.key_pressed(Key::Z) && !i.modifiers.shift {
                self.column_state.undo();
            }
            // Cmd+Shift+Z for redo column actions
            if i.modifiers.command && i.modifiers.shift && i.key_pressed(Key::Z) {
                self.column_state.redo();
            }
        });

        // Top panel with menu/toolbar
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui: &mut egui::Ui| {
                ui.menu_button("File", |ui: &mut egui::Ui| {
                    if ui.button("Open...").clicked() {
                        ui.close();
                        self.open_file(ctx);
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Edit", |ui: &mut egui::Ui| {
                    if ui.button("Find... (⌘+F)").clicked() {
                        ui.close();
                        self.search.visible = true;
                        self.search.focus_input = true;
                    }
                    let has_nav_rows = !self.search.results.read().navigation_rows.is_empty();
                    if ui
                        .add_enabled(has_nav_rows, egui::Button::new("Find Next (F3)"))
                        .clicked()
                    {
                        ui.close();
                        self.next_match();
                    }
                    if ui
                        .add_enabled(has_nav_rows, egui::Button::new("Find Previous (⇧F3)"))
                        .clicked()
                    {
                        ui.close();
                        self.prev_match();
                    }
                    ui.separator();
                    if ui.button("Go to Row... (⌘+L)").clicked() {
                        ui.close();
                        self.go_to_row.open = true;
                        self.go_to_row.focus_input = true;
                        self.go_to_row.input.clear();
                    }
                });
                ui.menu_button("View", |ui: &mut egui::Ui| {
                    if ui.button("Column Manager... (⌘+Shift+C)").clicked() {
                        ui.close();
                        self.column_state.manager_open = true;
                    }
                    ui.separator();
                    let theme_label = if self.dark_mode {
                        "☀ Light Mode"
                    } else {
                        "🌙 Dark Mode"
                    };
                    if ui.button(theme_label).clicked() {
                        self.dark_mode = !self.dark_mode;
                        if self.dark_mode {
                            ctx.set_visuals(egui::Visuals::dark());
                        } else {
                            // Custom light mode with better striped rows
                            let mut light = egui::Visuals::light();
                            // Make stripe color more visible (light grey)
                            light.faint_bg_color = Color32::from_rgb(235, 235, 240);
                            // Slightly darker widget background
                            light.widgets.noninteractive.bg_fill = Color32::from_rgb(245, 245, 248);
                            ctx.set_visuals(light);
                        }
                        ui.close();
                    }
                });
            });
        });

        // Search bar panel (shown below menu when search is active)
        if self.search.visible {
            egui::TopBottomPanel::top("search_panel")
                .exact_height(36.0)
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    self.render_search_bar(ui, ctx);
                });
        }

        // Update available banner
        if self.update_state.update_available.load(Ordering::Relaxed)
            && !self.update_state.dismissed
        {
            egui::TopBottomPanel::top("update_banner")
                .exact_height(32.0)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(Color32::from_rgb(40, 80, 40))
                        .show(ui, |ui| {
                            ui.horizontal_centered(|ui| {
                                ui.add_space(10.0);
                                let version = self.update_state.latest_version.read();
                                let version_str = version.as_deref().unwrap_or("new version");
                                ui.label(
                                    egui::RichText::new(format!(
                                        "⬆ Update available: v{version_str}"
                                    ))
                                    .color(Color32::WHITE),
                                );
                                ui.add_space(10.0);
                                if ui
                                    .add(egui::Button::new("Update in Terminal").small())
                                    .on_hover_text("Opens Terminal to run: brew update && brew upgrade")
                                    .clicked()
                                {
                                    // Open Terminal and run commands (update, upgrade, and restart app)
                                    #[cfg(not(target_arch = "wasm32"))]
                                    {
                                        let script = "tell application \"Terminal\" to do script \"brew update && brew upgrade --cask quickcsv && open -a QuickCSV\"";

                                        let success = std::process::Command::new("osascript")
                                            .arg("-e")
                                            .arg(script)
                                            .spawn()
                                            .is_ok();

                                        // Fallback: copy command to clipboard if Terminal automation fails
                                        if !success {
                                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                                let _ = clipboard
                                                    .set_text("brew update && brew upgrade --cask quickcsv");
                                            }
                                            // Show macOS notification to inform user
                                            let notify_script = "display notification \"Paste in Terminal to update\" with title \"QuickCSV\" subtitle \"Update command copied to clipboard\"";
                                            let _ = std::process::Command::new("osascript")
                                                .arg("-e")
                                                .arg(notify_script)
                                                .spawn();
                                        }

                                        // Close this instance so it can be overwritten and restarted
                                        std::process::exit(0);
                                    }
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.add_space(10.0);
                                        // Use unicode X for guaranteed rendering
                                        if ui.small_button("✕").clicked() {
                                            self.update_state.dismissed = true;
                                        }
                                    },
                                );
                            });
                        });
                });
        }

        // Bottom panel with status bar
        egui::TopBottomPanel::bottom("status_bar")
            .exact_height(28.0)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    self.render_status_bar(ui);
                });
            });

        // Central panel with table
        egui::CentralPanel::default().show(ctx, |ui| {
            let state = self.state.read();
            let load_state = state.load_state;
            drop(state);

            match load_state {
                LoadState::Empty => {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.heading("QuickCSV");
                            ui.add_space(20.0);
                            if ui.button("📂 Open CSV File").clicked() {
                                self.open_file(ctx);
                            }
                            ui.add_space(10.0);
                            ui.label("or drag and drop a file here");
                        });
                    });
                }
                LoadState::Indexing => {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.spinner();
                            ui.add_space(10.0);
                            let state = self.state.read();
                            let rows = state.rows_indexed.load(Ordering::Relaxed);
                            ui.label(format!(
                                "Indexing file... {} rows found",
                                format_number(rows)
                            ));
                        });
                    });
                    // Request continuous repaint during indexing
                    ctx.request_repaint();
                }
                LoadState::Ready => {
                    self.render_table(ui);
                }
                LoadState::Error => {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            let state = self.state.read();
                            ui.colored_label(
                                egui::Color32::RED,
                                format!(
                                    "Error: {}",
                                    state.error_message.as_deref().unwrap_or("Unknown")
                                ),
                            );
                            ui.add_space(20.0);
                            if ui.button("Try Again").clicked() {
                                drop(state);
                                self.open_file(ctx);
                            }
                        });
                    });
                }
            }
        });

        // Render JSON viewer popup (if open)
        self.render_json_popup(ctx);

        // Render Go to Row dialog (if open)
        self.render_go_to_row_dialog(ctx);

        // Render Row Detail popup (if open)
        self.render_row_detail_popup(ctx);

        // Render Column Manager dialog (if open)
        self.render_column_manager(ctx);

        // Render Filter popup (if open)
        self.render_filter_popup(ctx);

        // Handle dropped files
        ctx.input(|i| {
            for file in &i.raw.dropped_files {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(path) = &file.path {
                    self.load_file(path.clone(), ctx.clone());
                    break;
                }

                #[cfg(target_arch = "wasm32")]
                if let Some(bytes) = &file.bytes {
                    self.load_file_web(file.name.clone(), bytes.to_vec(), ctx.clone());
                    break;
                }
            }
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn load_icon() -> egui::IconData {
    let (icon_rgba, icon_width, icon_height) = {
        let icon_bytes = include_bytes!("../icons/icon-128.png");
        let image = image::load_from_memory(icon_bytes)
            .expect("Failed to load icon")
            .into_rgba8();
        let (width, height) = image.dimensions();
        (image.into_vec(), width, height)
    };

    egui::IconData {
        rgba: icon_rgba,
        width: icon_width,
        height: icon_height,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    let icon = load_icon();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([600.0, 400.0])
            .with_title("QuickCSV")
            .with_icon(icon)
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "QuickCSV",
        options,
        Box::new(|cc| {
            // Add Phosphor icons to fonts
            let mut fonts = egui::FontDefinitions::default();
            egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
            cc.egui_ctx.set_fonts(fonts);

            // Follow system theme
            cc.egui_ctx.set_visuals(egui::Visuals::dark());

            Ok(Box::new(FastCsvApp::default()))
        }),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    // Redirect panic to console log
    console_error_panic_hook::set_once();

    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window()
            .expect("No window")
            .document()
            .expect("No document");

        // Hide the loading spinner
        if let Some(loading_element) = document.get_element_by_id("center_text") {
            loading_element
                .set_attribute("style", "display: none;")
                .ok();
        }

        let canvas = document
            .get_element_by_id("the_canvas_id")
            .expect("Failed to find canvas element")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("Element is not a canvas");

        eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| {
                    // Add Phosphor icons to fonts
                    let mut fonts = egui::FontDefinitions::default();
                    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
                    cc.egui_ctx.set_fonts(fonts);

                    // Follow system theme
                    cc.egui_ctx.set_visuals(egui::Visuals::dark());

                    Ok(Box::new(FastCsvApp::default()))
                }),
            )
            .await
            .expect("failed to start eframe");
    });
}
