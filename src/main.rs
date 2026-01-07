//! QuickCSV - High-Performance CSV Viewer for macOS and Web
//!
//! A memory-mapped, virtualized CSV viewer that can handle files from 100MB to 2GB+
//! with zero lag. Uses memmap2 for zero-copy file loading and egui for the UI.

// Module declarations
mod csv;
mod state;
mod update;
mod utils;

use eframe::egui::{self, Color32, Key};

#[cfg(not(target_arch = "wasm32"))]
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
    ColumnState, FilterOperator, FilterState, LoadState, SearchStatus, SortDirection, SortState,
    TabState, MAX_NAV_ROWS,
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

/// Get current Unix timestamp (cross-platform)
fn current_timestamp() -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        // Use JavaScript Date API for WASM
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::UNDEFINED);
        (date.get_time() / 1000.0) as i64 // Convert from milliseconds to seconds
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Use standard library for native
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }
}

/// Height of each row in pixels
const ROW_HEIGHT: f32 = 24.0;

/// Highlight color for search matches
const HIGHLIGHT_COLOR: Color32 = Color32::from_rgb(255, 200, 0);

/// Highlight color for current/active search match
const CURRENT_MATCH_COLOR: Color32 = Color32::from_rgb(255, 120, 0);

/// Default column width
const DEFAULT_COLUMN_WIDTH: f32 = 150.0;

/// Maximum number of recent files to keep
const MAX_RECENT_FILES: usize = 20;

/// Recent file entry
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct RecentFile {
    /// File path (desktop) or file name (web)
    path: String,
    /// Display name (filename only)
    name: String,
    /// Last accessed timestamp (Unix timestamp)
    timestamp: i64,
}

/// Recent files state
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct RecentFiles {
    files: Vec<RecentFile>,
}

impl RecentFiles {
    /// Add a file to recent files list
    fn add_file(&mut self, path: String, name: String) {
        // Remove if already exists
        self.files.retain(|f| f.path != path);

        // Add to end (most recent)
        self.files.push(RecentFile {
            path,
            name,
            timestamp: current_timestamp(),
        });

        // Keep only most recent MAX_RECENT_FILES
        if self.files.len() > MAX_RECENT_FILES {
            self.files.remove(0);
        }
    }

    /// Remove a file from recent files
    #[allow(dead_code)] // Used in native code only
    fn remove_file(&mut self, path: &str) {
        self.files.retain(|f| f.path != path);
    }

    /// Get recent files sorted by most recent first
    fn get_recent(&self) -> Vec<RecentFile> {
        let mut files = self.files.clone();
        files.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        files
    }

    /// Validate and clean up recent files (remove non-existent files on desktop)
    #[cfg(not(target_arch = "wasm32"))]
    fn validate_desktop(&mut self) {
        self.files
            .retain(|f| std::path::Path::new(&f.path).exists());
    }
}

/// Main application state
struct FastCsvApp {
    /// Tabs (one per open file)
    tabs: Vec<TabState>,
    /// Index of currently active tab
    active_tab_index: usize,
    /// Dark mode enabled (true = dark, false = light) - shared across all tabs
    dark_mode: bool,
    /// Update checker state - shared across all tabs
    update_state: UpdateState,
    /// Recent files list - shared across all tabs
    recent_files: RecentFiles,
    /// Whether recent files have been loaded from storage
    recent_files_loaded: bool,
    /// Whether window has been expanded after file load
    #[allow(dead_code)] // Used in native code only
    window_expanded: bool,
    /// Whether keyboard shortcuts dialog is open
    shortcuts_dialog_open: bool,
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

        // Start with one empty tab
        let tabs = vec![TabState::new_empty()];

        Self {
            tabs,
            active_tab_index: 0,
            dark_mode: true, // Default to dark mode
            update_state: UpdateState::default(),
            recent_files: RecentFiles::default(),
            recent_files_loaded: false,
            window_expanded: false,
            shortcuts_dialog_open: false,
            #[cfg(target_arch = "wasm32")]
            file_loader_tx: tx,
            #[cfg(target_arch = "wasm32")]
            file_loader_rx: rx,
        }
    }
}

impl FastCsvApp {
    /// Get mutable reference to active tab
    fn active_tab_mut(&mut self) -> &mut TabState {
        &mut self.tabs[self.active_tab_index]
    }

    /// Get reference to active tab
    #[allow(dead_code)] // May be useful for future features
    fn active_tab(&self) -> &TabState {
        &self.tabs[self.active_tab_index]
    }

    /// Ensure we have at least one tab
    fn ensure_tabs(&mut self) {
        if self.tabs.is_empty() {
            self.tabs.push(TabState::new_empty());
            self.active_tab_index = 0;
        }
        // Ensure active_tab_index is valid
        if self.active_tab_index >= self.tabs.len() {
            self.active_tab_index = self.tabs.len().saturating_sub(1);
        }
    }

    /// Load recent files from storage
    #[cfg(not(target_arch = "wasm32"))]
    fn load_recent_files(&mut self) {
        if let Some(config_dir) = dirs::config_dir() {
            let recent_files_path = config_dir.join("quickcsv").join("recent_files.json");
            if recent_files_path.exists() {
                if let Ok(contents) = std::fs::read_to_string(&recent_files_path) {
                    if let Ok(files) = serde_json::from_str::<RecentFiles>(&contents) {
                        self.recent_files = files;
                        self.recent_files.validate_desktop();
                    }
                }
            }
        }
    }

    /// Save recent files to storage
    #[cfg(not(target_arch = "wasm32"))]
    fn save_recent_files(&self) {
        if let Some(config_dir) = dirs::config_dir() {
            let quickcsv_dir = config_dir.join("quickcsv");
            if std::fs::create_dir_all(&quickcsv_dir).is_err() {
                return;
            }
            let recent_files_path = quickcsv_dir.join("recent_files.json");
            if let Ok(json) = serde_json::to_string_pretty(&self.recent_files) {
                let _ = std::fs::write(&recent_files_path, json);
            }
        }
    }

    /// Load recent files from storage (web)
    #[cfg(target_arch = "wasm32")]
    fn load_recent_files(&mut self) {
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                if let Ok(Some(json)) = storage.get_item("quickcsv_recent_files") {
                    if let Ok(files) = serde_json::from_str::<RecentFiles>(&json) {
                        self.recent_files = files;
                    }
                }
            }
        }
    }

    /// Save recent files to storage (web)
    #[cfg(target_arch = "wasm32")]
    fn save_recent_files(&self) {
        if let Ok(json) = serde_json::to_string(&self.recent_files) {
            if let Some(window) = web_sys::window() {
                if let Ok(Some(storage)) = window.local_storage() {
                    let _ = storage.set_item("quickcsv_recent_files", &json);
                }
            }
        }
    }

    /// Open a file from recent files (desktop)
    #[cfg(not(target_arch = "wasm32"))]
    fn open_recent_file(&mut self, path: &str, ctx: &egui::Context) {
        let path_buf = PathBuf::from(path);
        if path_buf.exists() {
            self.load_file(path_buf, ctx.clone());
        } else {
            // Remove from recent files if it doesn't exist
            self.recent_files.remove_file(path);
            self.save_recent_files();
        }
    }

    /// Open a file from recent files (web) - triggers file picker
    #[cfg(target_arch = "wasm32")]
    fn open_recent_file(&mut self, _name: &str, ctx: &egui::Context) {
        // On web, we can't directly open files, so just trigger the file picker
        // The user will need to select the file again
        self.open_file(ctx);
    }

    /// Open a file dialog and queue the file for loading (WASM)
    #[cfg(target_arch = "wasm32")]
    fn open_file(&mut self, ctx: &egui::Context) {
        self.ensure_tabs();

        // Clone what we need before getting mutable borrow
        let tx = self.file_loader_tx.clone();
        let tab = self.active_tab_mut();
        let state = tab.state.clone();
        let ctx = ctx.clone();

        let task = rfd::AsyncFileDialog::new()
            .add_filter("CSV", &["csv", "tsv", "txt"])
            .pick_file();

        wasm_bindgen_futures::spawn_local(async move {
            if let Some(file) = task.await {
                // Show loading state only after user selects a file
                {
                    let mut state_guard = state.write();
                    state_guard.load_state = LoadState::Indexing;
                }
                ctx.request_repaint();

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
        self.ensure_tabs();
        let tab = self.active_tab_mut();

        // Reset state
        tab.scroll_y = 0.0;
        tab.scroll_x = 0.0;
        tab.column_widths.clear();
        tab.row_cache.clear();
        tab.last_visible_range = (0, 0);
        tab.sort_state = SortState::default();
        tab.column_state = ColumnState::default();
        tab.filter_state = FilterState::default();
        tab.filtered_indices = None;
        tab.applied_filters.clear();
        tab.applied_sort_column = None;
        tab.search.cancel_flag.store(true, Ordering::SeqCst);
        tab.search.current_index = 0;
        tab.search.active_query.clear();
        {
            let mut results = tab.search.results.write();
            results.navigation_rows.clear();
            results.total_match_count = 0;
            results.status = SearchStatus::Idle;
            results.rows_searched = 0;
            results.nav_limit_reached = false;
        }

        // Set loading state
        {
            let mut state_guard = tab.state.write();
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
        let state = Arc::clone(&tab.state);
        let result = csv::init_csv_web(name.clone(), bytes, &state, &ctx);

        if let Err(e) = result {
            let mut state_guard = state.write();
            state_guard.load_state = LoadState::Error;
            state_guard.error_message = Some(e);
            ctx.request_repaint();
        } else {
            // Update tab file info
            tab.file_path = name.clone();
            tab.file_name = name.clone();
            // Add to recent files
            self.recent_files.add_file(name.clone(), name);
            self.save_recent_files();
        }
    }

    /// Open a file dialog and load the selected CSV file (Native)
    #[cfg(not(target_arch = "wasm32"))]
    fn open_file(&mut self, ctx: &egui::Context) {
        self.ensure_tabs();
        let tab = self.active_tab_mut();

        // Cancel any ongoing indexing
        {
            let state = tab.state.read();
            state.cancel_indexing.store(true, Ordering::Relaxed);
        }

        // Open native file dialog
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("CSV Files", &["csv", "tsv", "txt"])
            .add_filter("All Files", &["*"])
            .pick_file()
        {
            // Create new tab for the file
            let new_tab = TabState::from_path(path.clone());
            self.tabs.push(new_tab);
            self.active_tab_index = self.tabs.len() - 1;
            self.load_file(path, ctx.clone());
        }
    }

    /// Load a CSV file in the background (Native)
    #[cfg(not(target_arch = "wasm32"))]
    fn load_file(&mut self, path: PathBuf, ctx: egui::Context) {
        self.ensure_tabs();
        let tab = self.active_tab_mut();

        // Reset state
        tab.scroll_y = 0.0;
        tab.scroll_x = 0.0;
        tab.column_widths.clear();
        tab.row_cache.clear();
        tab.last_visible_range = (0, 0);
        // Reset sort state
        tab.sort_state = SortState::default();
        // Reset column state (visibility, order)
        tab.column_state = ColumnState::default();
        // Reset filter state
        tab.filter_state = FilterState::default();
        tab.filtered_indices = None;
        tab.filter_version = 0;
        tab.last_filter_version = 0;
        tab.filter_receiver = None;
        tab.is_filtering.store(false, Ordering::Relaxed);
        tab.applied_filters.clear();
        tab.applied_sort_column = None;
        tab.applied_sort_direction = SortDirection::None;
        tab.filter_duration = None;
        // Cancel any ongoing search and clear results
        tab.search.cancel_flag.store(true, Ordering::SeqCst);
        tab.search.current_index = 0;
        tab.search.active_query.clear();
        {
            let mut results = tab.search.results.write();
            results.navigation_rows.clear();
            results.total_match_count = 0;
            results.status = SearchStatus::Idle;
            results.rows_searched = 0;
            results.nav_limit_reached = false;
        }

        // Set loading state
        {
            let mut state = tab.state.write();
            state.csv = None;
            state.load_state = LoadState::Indexing;
            state.error_message = None;
            state.rows_indexed.store(0, Ordering::Relaxed);
            state.cancel_indexing.store(false, Ordering::Relaxed);
            state.indexing_complete.store(false, Ordering::Relaxed);
        }

        let state = Arc::clone(&tab.state);
        let path_clone = path.clone();
        let path_str = path.to_string_lossy().to_string();
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&path_str)
            .to_string();

        // Update tab file info
        tab.file_path = path_str.clone();
        tab.file_name = file_name.clone();

        // Add to recent files
        self.recent_files.add_file(path_str, file_name);
        self.save_recent_files();

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
        self.ensure_tabs();
        let tab = self.active_tab_mut();
        tab.filter_version = tab.filter_version.wrapping_add(1);
        // Don't clear indices/cache here to avoid flashing content.
        // They will be updated when async task completes.
    }

    /// Start async filtering task
    fn start_async_filtering(&mut self, _ctx: &egui::Context) {
        self.ensure_tabs();
        let tab = self.active_tab_mut();

        // If no filters are active, clear filtered indices immediately
        if !tab.filter_state.has_active_filters() {
            tab.filtered_indices = None;
            tab.last_filter_version = tab.filter_version;
            tab.applied_filters.clear(); // Clear applied state
            return;
        }

        // Check if we are already filtering
        if tab.is_filtering.load(Ordering::Relaxed) {
            // If already filtering, maybe we should cancel previous?
            // For now, simple implementation: just start new one, late comer wins visually
            // but cleaner would be cancellation.
        }

        tab.is_filtering.store(true, Ordering::Relaxed);
        tab.last_filter_version = tab.filter_version;

        // Clone state for thread
        let state = tab.state.clone();
        let filter_state = tab.filter_state.clone();

        // Snapshot current state for optimization check and return
        let current_filters = tab.filter_state.filters.clone();
        let sort_col = tab.sort_state.column;
        let sort_dir = tab.sort_state.direction;

        // Check for optimization:
        let can_optimize = tab.filtered_indices.is_some()
            && tab.applied_sort_column == sort_col
            && tab.applied_sort_direction == sort_dir
            && current_filters.len() > tab.applied_filters.len()
            && tab
                .applied_filters
                .iter()
                .all(|(k, v)| current_filters.get(k) == Some(v));

        let initial_indices = if can_optimize {
            tab.filtered_indices.clone()
        } else {
            None
        };

        // Capture sort state for consistent ordering
        let sorted_indices = {
            let guard = tab.sort_state.sorted_indices.read();
            if tab.sort_state.direction != SortDirection::None && !guard.is_empty() {
                Some(guard.clone())
            } else {
                None
            }
        };

        // Create channel
        let (sender, receiver) = mpsc::channel();
        tab.filter_receiver = Some(receiver);

        let is_filtering = tab.is_filtering.clone();

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
        self.ensure_tabs();
        let tab = self.active_tab_mut();

        // If already sorting, ignore click
        if tab.sort_state.is_sorting.load(Ordering::Relaxed) {
            return;
        }

        // Determine new sort direction
        let new_direction = if tab.sort_state.column == Some(col_idx) {
            // Same column - cycle through directions
            match tab.sort_state.direction {
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
            tab.sort_state.cancel_flag.store(true, Ordering::SeqCst);
            tab.sort_state = SortState::default();
            tab.row_cache.clear();
            return;
        }

        // Cancel any previous sort
        tab.sort_state.cancel_flag.store(true, Ordering::SeqCst);

        // Update sort state
        tab.sort_state.column = Some(col_idx);
        tab.sort_state.direction = new_direction;
        tab.sort_state.is_sorting.store(true, Ordering::SeqCst);
        tab.sort_state.progress.store(0, Ordering::SeqCst);
        tab.sort_state.cancel_flag = Arc::new(AtomicBool::new(false));

        // Clear sorted indices and row cache
        {
            let mut indices = tab.sort_state.sorted_indices.write();
            indices.clear();
        }
        tab.row_cache.clear();

        // Clone what we need for the background thread
        let state = Arc::clone(&tab.state);
        let sorted_indices = Arc::clone(&tab.sort_state.sorted_indices);
        let is_sorting = Arc::clone(&tab.sort_state.is_sorting);
        let progress = Arc::clone(&tab.sort_state.progress);
        let cancel_flag = Arc::clone(&tab.sort_state.cancel_flag);
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
    #[allow(dead_code)] // May be useful for future features
    fn get_actual_row_index(&self, display_index: usize) -> usize {
        // Note: ensure_tabs requires &mut, but this method only needs &self
        // We'll assume tabs are already ensured by the caller
        if self.tabs.is_empty() {
            return display_index;
        }
        let tab = &self.tabs[self.active_tab_index];
        if tab.sort_state.direction == SortDirection::None {
            return display_index;
        }

        let indices = tab.sort_state.sorted_indices.read();
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
        self.ensure_tabs();
        let tab = self.active_tab_mut();
        let query = tab.search.query.trim().to_lowercase();

        // Don't search if query is empty
        if query.is_empty() {
            tab.search.cancel_flag.store(true, Ordering::SeqCst);
            let mut results = tab.search.results.write();
            results.navigation_rows.clear();
            results.total_match_count = 0;
            results.status = SearchStatus::Idle;
            return;
        }

        // Add to search history (if not duplicate of last entry)
        let query_for_history = tab.search.query.trim().to_string();
        if !query_for_history.is_empty() {
            // Remove if already exists (to move to end)
            tab.search.history.retain(|h| h != &query_for_history);
            tab.search.history.push(query_for_history);
            // Keep history limited to last 50 entries
            if tab.search.history.len() > 50 {
                tab.search.history.remove(0);
            }
        }
        // Reset history navigation
        tab.search.history_index = None;

        // Cancel any previous search
        tab.search.cancel_flag.store(true, Ordering::SeqCst);

        // Create new cancel flag for this search
        tab.search.cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_flag = Arc::clone(&tab.search.cancel_flag);

        // Reset search state
        tab.search.current_index = 0;
        tab.search.active_query = query.clone();

        // Get total rows and visible columns for progress
        let (total_rows, visible_columns) = {
            let num_columns = {
                let state = tab.state.read();
                state.csv.as_ref().map(|c| c.headers.len()).unwrap_or(0)
            };

            // Initialize column order if needed
            if num_columns > 0 {
                tab.column_state.init_column_order(num_columns);
            }

            // Get visible columns (original indices)
            let visible = tab.column_state.get_visible_columns();

            // Get total rows
            let total = {
                let state = tab.state.read();
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
            let mut results = tab.search.results.write();
            results.navigation_rows.clear();
            results.total_match_count = 0;
            results.status = SearchStatus::Searching;
            results.rows_searched = 0;
            results.total_rows = total_rows;
            results.nav_limit_reached = false;
        }

        // Clone what we need for the background thread
        let state = Arc::clone(&tab.state);
        let results = Arc::clone(&tab.search.results);
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
        self.ensure_tabs();
        let tab = self.active_tab_mut();
        let results = tab.search.results.read();
        if results.navigation_rows.is_empty() {
            return;
        }
        tab.search.current_index = (tab.search.current_index + 1) % results.navigation_rows.len();
        let row = results.navigation_rows[tab.search.current_index];
        tab.search.scroll_to_row = Some(row);
    }

    /// Navigate to previous matching row
    fn prev_match(&mut self) {
        self.ensure_tabs();
        let tab = self.active_tab_mut();
        let results = tab.search.results.read();
        if results.navigation_rows.is_empty() {
            return;
        }
        if tab.search.current_index == 0 {
            tab.search.current_index = results.navigation_rows.len() - 1;
        } else {
            tab.search.current_index -= 1;
        }
        let row = results.navigation_rows[tab.search.current_index];
        tab.search.scroll_to_row = Some(row);
    }

    /// Create a new empty tab and make it active
    fn new_tab(&mut self) {
        self.tabs.push(TabState::new_empty());
        self.active_tab_index = self.tabs.len() - 1;
    }

    /// Close the active tab (only if more than one tab exists)
    fn close_active_tab(&mut self) {
        if self.tabs.len() <= 1 {
            return; // Don't close the last tab
        }

        let idx = self.active_tab_index;

        // Adjust active tab index
        if idx > 0 {
            self.active_tab_index = idx - 1;
        } else if self.tabs.len() > 1 {
            self.active_tab_index = 0;
        }

        // Remove the tab
        self.tabs.remove(idx);
    }

    /// Render the tab bar (compact, minimal height)
    fn render_tab_bar(&mut self, ui: &mut egui::Ui) {
        self.ensure_tabs();

        // Compact spacing for a slim bar
        let spacing = ui.spacing_mut();
        spacing.item_spacing = egui::vec2(6.0, 0.0);
        spacing.button_padding = egui::vec2(8.0, 4.0);

        // Collect tabs to close after UI rendering
        let mut tabs_to_close = Vec::new();

        ui.horizontal(|ui| {
            // Tab strip with hidden scrollbar to avoid extra height
            egui::ScrollArea::horizontal()
                .id_salt("tab_strip")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .max_height(24.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for idx in 0..self.tabs.len() {
                            let is_active = idx == self.active_tab_index;
                            let tab = &self.tabs[idx];
                            let raw_title = if tab.file_name.is_empty() {
                                "New Tab".to_string()
                            } else {
                                tab.file_name.clone()
                            };
                            // Truncate very long titles so first tab doesn't eat the bar
                            let title = if raw_title.len() > 32 {
                                format!("{}…", &raw_title[..29])
                            } else {
                                raw_title
                            };

                            // Tab button
                            let label = egui::RichText::new(format!(
                                "{} {}",
                                egui_phosphor::regular::FILE_TEXT,
                                title
                            ))
                            .size(13.0);
                            let response = ui.selectable_label(is_active, label).on_hover_text(
                                if tab.state.read().csv.is_some() {
                                    format!("Switch to: {}", tab.file_name)
                                } else {
                                    "New Tab".to_string()
                                },
                            );

                            if response.clicked() {
                                self.active_tab_index = idx;
                            }

                            // Close button (only show if more than one tab)
                            if self.tabs.len() > 1
                                && ui
                                    .small_button(egui_phosphor::regular::X)
                                    .on_hover_text("Close tab")
                                    .clicked()
                            {
                                tabs_to_close.push(idx);
                            }
                        }
                    });
                });

            ui.add_space(4.0);

            // New tab button (compact)
            if ui
                .small_button(egui_phosphor::regular::PLUS)
                .on_hover_text("Open new tab (Cmd+T)")
                .clicked()
            {
                self.new_tab();
            }
        });

        // Close tabs (in reverse order to maintain indices)
        tabs_to_close.sort_unstable();
        tabs_to_close.reverse();
        for idx in tabs_to_close {
            if self.tabs.len() <= 1 {
                break;
            }
            if idx < self.active_tab_index {
                self.active_tab_index -= 1;
            } else if idx == self.active_tab_index {
                // If closing active tab, switch to previous or next
                if idx > 0 {
                    self.active_tab_index = idx - 1;
                } else if self.tabs.len() > 1 {
                    self.active_tab_index = 0;
                }
            }
            self.tabs.remove(idx);
        }
    }

    /// Render the search bar
    fn render_search_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.ensure_tabs();
        let tab_idx = self.active_tab_index;

        // Read search results state and query without holding borrow
        let (
            status,
            total_matches,
            nav_row_count,
            rows_searched,
            total_rows,
            nav_limit_reached,
            search_query,
            current_index,
            active_query,
        ) = {
            let tab = &self.tabs[tab_idx];
            let results = tab.search.results.read();
            (
                results.status,
                results.total_match_count,
                results.navigation_rows.len(),
                results.rows_searched,
                results.total_rows,
                results.nav_limit_reached,
                tab.search.query.clone(),
                tab.search.current_index,
                tab.search.active_query.clone(),
            )
        };

        // Access tab inside closure using tab_idx
        let _query_changed = false;
        let _new_query = search_query.clone();
        let mut should_focus = false;
        let _history_nav: Option<(bool, bool)> = None; // (up, down)
        let mut should_search = false;
        let mut should_cancel = false;
        let mut should_close = false;
        let mut nav_prev = false;
        let mut nav_next = false;

        ui.horizontal(|ui| {
            ui.label("🔍");

            // Get mutable access to tab inside closure
            let tab = &mut self.tabs[tab_idx];

            // Auto-focus when search bar opens
            if tab.search.focus_input {
                should_focus = true;
                tab.search.focus_input = false;
            }

            // Search input field
            let response = ui.add(
                egui::TextEdit::singleline(&mut tab.search.query)
                    .hint_text("Search visible columns...")
                    .desired_width(250.0),
            );

            if should_focus {
                response.request_focus();
            }

            // Handle up/down arrow for history navigation (only when focused)
            if response.has_focus() && !tab.search.history.is_empty() {
                let up_pressed = ui.input(|i| i.key_pressed(Key::ArrowUp));
                let down_pressed = ui.input(|i| i.key_pressed(Key::ArrowDown));

                if up_pressed {
                    match tab.search.history_index {
                        None => {
                            // Save current query and start browsing from most recent
                            tab.search.history_temp_query = tab.search.query.clone();
                            tab.search.history_index = Some(tab.search.history.len() - 1);
                            tab.search.query =
                                tab.search.history.last().cloned().unwrap_or_default();
                        }
                        Some(idx) if idx > 0 => {
                            // Go to older entry
                            tab.search.history_index = Some(idx - 1);
                            tab.search.query = tab.search.history[idx - 1].clone();
                        }
                        _ => {} // Already at oldest entry
                    }
                }

                if down_pressed {
                    match tab.search.history_index {
                        Some(idx) if idx < tab.search.history.len() - 1 => {
                            // Go to newer entry
                            tab.search.history_index = Some(idx + 1);
                            tab.search.query = tab.search.history[idx + 1].clone();
                        }
                        Some(_) => {
                            // Back to the original query
                            tab.search.history_index = None;
                            tab.search.query = tab.search.history_temp_query.clone();
                        }
                        None => {} // Not browsing history
                    }
                }
            }

            // Execute search on Enter (keep focus on input)
            let enter_pressed = ui.input(|i| i.key_pressed(Key::Enter));
            let should_search_enter =
                enter_pressed && (response.has_focus() || response.lost_focus());

            // Search button (disabled during search)
            let is_searching = status == SearchStatus::Searching;
            let should_search_button = ui
                .add_enabled(!is_searching, egui::Button::new("Search"))
                .clicked();

            // Cancel button during search
            should_cancel = is_searching && ui.button("Cancel").clicked();

            // Collect search action
            should_search = should_search_enter || should_search_button;

            ui.separator();

            // Navigation buttons (navigate through rows with matches)
            let has_nav_rows = nav_row_count > 0;

            nav_prev = ui
                .add_enabled(has_nav_rows && !is_searching, egui::Button::new("◀"))
                .clicked();
            nav_next = ui
                .add_enabled(has_nav_rows && !is_searching, egui::Button::new("▶"))
                .clicked();

            // Status display
            let is_searching_done =
                status == SearchStatus::Searching && total_rows > 0 && rows_searched >= total_rows;

            match status {
                SearchStatus::Searching => {
                    if is_searching_done {
                        ui.label(format!("Done ({} matches)", format_number(total_matches)));
                    } else {
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
                }
                SearchStatus::Complete | SearchStatus::Cancelled => {
                    if total_matches > 0 {
                        // Ensure current_index is valid
                        if current_index >= nav_row_count && nav_row_count > 0 {
                            tab.search.current_index = 0;
                        }
                        // Show total matches and navigation position
                        ui.label(format!("{} matches", format_number(total_matches)));
                        if has_nav_rows {
                            let display_index =
                                if current_index >= nav_row_count && nav_row_count > 0 {
                                    0
                                } else {
                                    current_index
                                };
                            ui.label(format!(
                                "(row {} of {}{})",
                                display_index + 1,
                                format_number(nav_row_count),
                                if nav_limit_reached { "+" } else { "" }
                            ));
                        }
                        if status == SearchStatus::Cancelled {
                            ui.label("(partial)");
                        }
                    } else if !active_query.is_empty() {
                        ui.label("No matches");
                    }
                }
                SearchStatus::Idle => {
                    // Show nothing
                }
            }

            // Request repaint during search to update progress
            if status == SearchStatus::Searching && !is_searching_done {
                ctx.request_repaint();
            }

            should_close = {
                let mut clicked = false;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    clicked = ui.button(egui_phosphor::regular::X).clicked();
                });
                clicked
            };
        });

        // Handle actions after UI closure (to avoid borrow conflicts)
        if should_search {
            self.execute_search(ctx);
        } else if should_cancel {
            let tab = &mut self.tabs[tab_idx];
            tab.search.cancel_flag.store(true, Ordering::SeqCst);
        } else if should_close {
            let tab = &mut self.tabs[tab_idx];
            tab.search.cancel_flag.store(true, Ordering::SeqCst);
            tab.search.visible = false;
            tab.search.query.clear();
            tab.search.active_query.clear();
            let mut results = tab.search.results.write();
            results.navigation_rows.clear();
            results.total_match_count = 0;
            results.status = SearchStatus::Idle;
        }

        if nav_prev {
            self.prev_match();
        }
        if nav_next {
            self.next_match();
        }

        // Request repaint during search to update progress
        let is_searching_done =
            status == SearchStatus::Searching && total_rows > 0 && rows_searched >= total_rows;
        if status == SearchStatus::Searching && !is_searching_done {
            ctx.request_repaint();
        }
    }

    /// Render the virtualized table using egui_extras::TableBuilder
    fn render_table(&mut self, ui: &mut egui::Ui) {
        self.ensure_tabs();
        // Get tab index - render_table_with_tab will access it directly
        let tab_idx = self.active_tab_index;
        self.render_table_with_tab(ui, tab_idx);
    }

    fn render_table_with_tab(&mut self, ui: &mut egui::Ui, tab_idx: usize) {
        use egui_extras::{Column, TableBuilder};

        // Extract data from state first, then drop the lock
        let (headers, total_rows, num_columns, _is_indexing) = {
            let tab = &self.tabs[tab_idx];
            let state = tab.state.read();
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
        {
            let tab = &mut self.tabs[tab_idx];
            tab.total_row_count = total_rows;
        }

        // Check if sorting just finished and filter state
        let (is_sorting, was_sorting, filter_changed) = {
            let tab = &self.tabs[tab_idx];
            (
                tab.sort_state.is_sorting.load(Ordering::Relaxed),
                tab.was_sorting,
                tab.filter_version != tab.last_filter_version,
            )
        };

        // Update was_sorting and handle filter changes
        {
            let tab = &mut self.tabs[tab_idx];
            tab.was_sorting = is_sorting;
        }

        if was_sorting && !is_sorting {
            // Sorting finished, need to recompute filtered indices in sorted order
            self.mark_filter_changed();
        }

        // Recompute filtered indices if filter changed
        if filter_changed {
            self.start_async_filtering(ui.ctx());
        }

        // Extract data we need before the closure
        let (
            display_rows,
            visible_columns,
            column_widths,
            scroll_to_row,
            search_query,
            current_nav_row,
            current_sort_col,
            current_sort_dir,
            is_sorting,
            sort_progress,
        ) = {
            let tab = &mut self.tabs[tab_idx];

            // Determine effective row count (filtered or total)
            let display_rows = match &tab.filtered_indices {
                Some(indices) => indices.len(),
                None => total_rows,
            };

            // Initialize column state if needed
            tab.column_state.init_column_order(num_columns);

            // Get visible columns in display order
            let visible_columns = tab.column_state.get_visible_columns().clone();
            let num_visible = visible_columns.len();

            // Ensure we have column widths for visible columns only
            if tab.column_widths.len() != num_visible {
                tab.column_widths = vec![DEFAULT_COLUMN_WIDTH; num_visible];
            }

            // Handle scroll to row request (from search or go-to-row dialog)
            let scroll_to_row = tab
                .search
                .scroll_to_row
                .take()
                .or_else(|| tab.go_to_row.scroll_to_row.take());

            // Get search query and current navigation row for highlighting
            let (search_query, current_nav_row) = {
                let results = tab.search.results.read();
                let nav_row = if !results.navigation_rows.is_empty()
                    && tab.search.current_index < results.navigation_rows.len()
                {
                    Some(results.navigation_rows[tab.search.current_index])
                } else {
                    None
                };
                (tab.search.active_query.clone(), nav_row)
            };

            let current_sort_col = tab.sort_state.column;
            let current_sort_dir = tab.sort_state.direction;
            let is_sorting = tab.sort_state.is_sorting.load(Ordering::Relaxed);
            let sort_progress = tab.sort_state.progress.load(Ordering::Relaxed);

            (
                display_rows,
                visible_columns,
                tab.column_widths.clone(),
                scroll_to_row,
                search_query,
                current_nav_row,
                current_sort_col,
                current_sort_dir,
                is_sorting,
                sort_progress,
            )
        };

        // Snapshot sorted indices (if any) to avoid borrowing self inside closures
        let sorted_indices = {
            let tab = &self.tabs[tab_idx];
            let guard = tab.sort_state.sorted_indices.read();
            if tab.sort_state.direction != SortDirection::None && !guard.is_empty() {
                Some(guard.clone())
            } else {
                None
            }
        };

        // Track actions that need to happen after the closure
        let mut clicked_column: Option<usize> = None;
        let mut row_to_open_detail: Option<usize> = None;
        let mut drop_target_idx: Option<usize> = None;

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
                for col_width in &column_widths {
                    table = table.column(Column::initial(*col_width).resizable(true).clip(true));
                }

                // Access tab inside closure
                let tab = &mut self.tabs[tab_idx];

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
                                        tab.column_state.dragged_column = Some(display_idx);
                                    }

                                    // Check drag state
                                    let is_dragging = tab.column_state.dragged_column.is_some();
                                    let is_being_dragged = tab.column_state.dragged_column == Some(display_idx);

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
                                        && tab.column_state.dragged_column.is_none()
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
                                    let has_filter = tab.filter_state.has_filter(original_idx);
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
                                        if tab.filter_state.active_popup == Some(original_idx) {
                                            tab.filter_state.close_popup();
                                        } else {
                                            tab.filter_state.open_popup(original_idx);
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
                            let actual_row_idx = if let Some(indices) = &tab.filtered_indices {
                                *indices.get(display_idx).unwrap_or(&0)
                            } else if let Some(sorted) = &sorted_indices {
                                *sorted.get(display_idx).unwrap_or(&display_idx)
                            } else {
                                display_idx
                            };

                            // Get or parse row data (cache by actual row index)
                            let fields = if let Some(cached) = tab.row_cache.get(&actual_row_idx) {
                                cached.clone()
                            } else {
                                // Parse from mmap
                                let state = tab.state.read();
                                let fields = if let Some(csv) = &state.csv {
                                    csv.parse_row(actual_row_idx).unwrap_or_default()
                                } else {
                                    vec![]
                                };
                                drop(state);

                                // Cache for next render
                                tab.row_cache.insert(actual_row_idx, fields.clone());
                                fields
                            };

                            // Check if this is the current navigation row (for special highlighting)
                            let is_current_nav_row = current_nav_row == Some(actual_row_idx);

                            // Check if this row is highlighted from "Go to Row"
                            let is_goto_highlighted = tab.go_to_row.highlight_row == Some(actual_row_idx);

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
                                        tab.go_to_row.highlight_row = None;
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

                                        tab.json_viewer.open = true;
                                        tab.json_viewer.raw_content = field.clone();
                                        tab.json_viewer.formatted_content =
                                            formatted.clone().unwrap_or_else(|| field.clone());
                                        tab.json_viewer.is_valid_json =
                                            is_json && formatted.is_some();
                                        tab.json_viewer.row = actual_row_idx;
                                        tab.json_viewer.col = original_idx; // Store original index
                                        tab.json_viewer.column_name = col_name;
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

            }); // End of horizontal scroll area

        // Handle actions after closure (to avoid borrow conflicts)
        // Handle column header click for sorting
        if let Some(col_idx) = clicked_column {
            self.sort_by_column(col_idx, ui.ctx());
        }

        // Handle column drop for reordering
        {
            let tab = &mut self.tabs[tab_idx];
            let pointer_released = ui.input(|i| i.pointer.any_released());
            if pointer_released {
                if let Some(dragged) = tab.column_state.dragged_column {
                    if let Some(target) = drop_target_idx {
                        if dragged != target {
                            tab.column_state.reorder_visible_columns(dragged, target);
                        }
                    }
                    tab.column_state.dragged_column = None;
                }
            }
        }

        // Handle row number double-click for row detail popup
        if let Some(row_idx) = row_to_open_detail {
            self.open_row_detail(row_idx);
        }

        // Prune old cache entries (keep last 2000 rows in cache)
        {
            let tab = &mut self.tabs[tab_idx];
            if tab.row_cache.len() > 2000 {
                let keys: Vec<usize> = tab.row_cache.keys().cloned().collect();
                for key in keys.into_iter().take(tab.row_cache.len() - 1000) {
                    tab.row_cache.remove(&key);
                }
            }
        }
    }

    /// Render the status bar
    fn render_status_bar(&self, ui: &mut egui::Ui) {
        // Note: ensure_tabs requires &mut, but this method only needs &self
        // We'll assume tabs are already ensured by the caller
        if self.tabs.is_empty() {
            ui.label("No file loaded");
            return;
        }
        let tab = &self.tabs[self.active_tab_index];
        let state = tab.state.read();

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
                    let is_filtering = tab.is_filtering.load(Ordering::Relaxed);

                    if is_still_indexing {
                        ui.spinner();
                        ui.label(format!("Rows: {}...", format_number(row_count)));
                    } else if is_filtering {
                        ui.spinner();
                        ui.label(format!("Filtering... (Rows: {})", format_number(row_count)));
                    } else if let Some(filtered) = &tab.filtered_indices {
                        let duration_text = if let Some(d) = tab.filter_duration {
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
        self.ensure_tabs();
        let tab = self.active_tab_mut();

        if !tab.json_viewer.open {
            return;
        }

        // Handle Escape to close
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            tab.json_viewer.open = false;
            return;
        }

        // Copy values to avoid borrow issues
        let column_name = tab.json_viewer.column_name.clone();
        let row_num = format_number(tab.json_viewer.row + 1);
        let is_valid = tab.json_viewer.is_valid_json;
        let formatted = tab.json_viewer.formatted_content.clone();
        let raw = tab.json_viewer.raw_content.clone();

        let mut should_close = false;

        // Determine title based on content type
        let title = if is_valid {
            format!("{} Cell Viewer (JSON)", egui_phosphor::regular::FILE_CODE)
        } else {
            format!("{} Cell Viewer", egui_phosphor::regular::FILE_TEXT)
        };

        egui::Window::new(title)
            .default_size([550.0, 450.0])
            .min_size([400.0, 300.0])
            .resizable(true)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(12.0);

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
            tab.json_viewer.open = false;
        }
    }

    /// Render the Go to Row dialog
    fn render_go_to_row_dialog(&mut self, ctx: &egui::Context) {
        self.ensure_tabs();
        let tab = self.active_tab_mut();

        if !tab.go_to_row.open {
            return;
        }

        // Get total rows for validation
        let total_rows = {
            let state = tab.state.read();
            state
                .csv
                .as_ref()
                .map(|c| c.indexed_row_count())
                .unwrap_or(0)
        };

        if total_rows == 0 {
            tab.go_to_row.open = false;
            return;
        }

        let mut should_close = false;
        let mut should_go = false;

        egui::Window::new(format!(
            "{} Go to Row",
            egui_phosphor::regular::ARROW_CIRCLE_RIGHT
        ))
        .default_size([380.0, 140.0])
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.add_space(12.0);

            ui.vertical(|ui| {
                ui.label(egui::RichText::new("Row number").color(Color32::from_rgb(200, 200, 200)));
                ui.add_space(6.0);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut tab.go_to_row.input)
                        .hint_text(format!("1 - {}", format_number(total_rows)))
                        .desired_width(350.0),
                );

                // Auto-focus on open
                if tab.go_to_row.focus_input {
                    response.request_focus();
                    tab.go_to_row.focus_input = false;
                }

                // Handle Enter key
                if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    should_go = true;
                }
            });

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(12.0);

            // Buttons - primary action on right
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(format!("{} Go", egui_phosphor::regular::CHECK))
                        .clicked()
                    {
                        should_go = true;
                    }
                    if ui
                        .button(format!("{} Cancel", egui_phosphor::regular::X))
                        .clicked()
                        || ui.input(|i| i.key_pressed(Key::Escape))
                    {
                        should_close = true;
                    }
                });
            });
            ui.add_space(10.0);
        });

        if should_go {
            // Parse and validate row number
            if let Ok(row_num) = tab.go_to_row.input.trim().replace(',', "").parse::<usize>() {
                if row_num >= 1 && row_num <= total_rows {
                    // Convert to 0-indexed
                    let row_idx = row_num - 1;
                    tab.go_to_row.scroll_to_row = Some(row_idx);
                    tab.go_to_row.highlight_row = Some(row_idx);
                    should_close = true;
                }
            }
        }

        if should_close {
            tab.go_to_row.open = false;
        }
    }

    /// Open row detail popup for a specific row
    fn open_row_detail(&mut self, row_index: usize) {
        self.ensure_tabs();
        let tab = self.active_tab_mut();

        // Get the row data and headers
        let (fields, headers) = {
            let state = tab.state.read();
            if let Some(csv) = &state.csv {
                let fields = csv.parse_row(row_index).unwrap_or_default();
                let headers = csv.headers.clone();
                (fields, headers)
            } else {
                return;
            }
        };

        tab.row_detail.open = true;
        tab.row_detail.row_index = row_index;
        tab.row_detail.fields = fields;
        tab.row_detail.headers = headers;
        tab.row_detail.expanded_fields.clear();
    }

    /// Render the row detail popup
    fn render_row_detail_popup(&mut self, ctx: &egui::Context) {
        self.ensure_tabs();
        let tab = self.active_tab_mut();

        if !tab.row_detail.open {
            return;
        }

        // Handle Escape to close
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            tab.row_detail.open = false;
            return;
        }

        let row_num = format_number(tab.row_detail.row_index + 1);
        let num_fields = tab.row_detail.fields.len();

        let mut should_close = false;
        let mut open_json_for: Option<(usize, String, String)> = None;
        let mut toggle_expand: Option<usize> = None;

        egui::Window::new(format!(
            "{} Row {} Details",
            egui_phosphor::regular::LIST_BULLETS,
            row_num
        ))
        .default_size([600.0, 500.0])
        .min_size([400.0, 300.0])
        .resizable(true)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.add_space(12.0);

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
                            let csv_row = tab
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
                            for (i, field) in tab.row_detail.fields.iter().enumerate() {
                                let header = tab
                                    .row_detail
                                    .headers
                                    .get(i)
                                    .cloned()
                                    .unwrap_or_else(|| format!("col_{}", i));
                                json_obj.insert(header, serde_json::Value::String(field.clone()));
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
                    for (i, field) in tab.row_detail.fields.iter().enumerate() {
                        let header = tab
                            .row_detail
                            .headers
                            .get(i)
                            .cloned()
                            .unwrap_or_else(|| format!("Column {}", i + 1));
                        let is_expanded = tab.row_detail.expanded_fields.contains(&i);
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
                                            egui::RichText::new(format!("({} chars)", field.len()))
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
                                                if let Ok(mut clipboard) = arboard::Clipboard::new()
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
                                                    tab.row_detail.row_index,
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
                                        .desired_rows(if is_large && is_expanded { 8 } else { 1 })
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
            if tab.row_detail.expanded_fields.contains(&idx) {
                tab.row_detail.expanded_fields.remove(&idx);
            } else {
                tab.row_detail.expanded_fields.insert(idx);
            }
        }

        // Handle opening JSON viewer
        if let Some((row, col_name, value)) = open_json_for {
            tab.row_detail.open = false;
            tab.json_viewer.row = row;
            tab.json_viewer.col = 0;
            tab.json_viewer.column_name = col_name;
            tab.json_viewer.raw_content = value.clone();
            if let Some(formatted) = format_json(&value) {
                tab.json_viewer.formatted_content = formatted;
                tab.json_viewer.is_valid_json = true;
            } else {
                tab.json_viewer.formatted_content = value;
                tab.json_viewer.is_valid_json = false;
            }
            tab.json_viewer.open = true;
        }

        if should_close {
            tab.row_detail.open = false;
        }
    }

    /// Render the filter popup for a column
    fn render_filter_popup(&mut self, ctx: &egui::Context) {
        self.ensure_tabs();
        let tab = self.active_tab_mut();

        let Some(col_idx) = tab.filter_state.active_popup else {
            return;
        };

        // Get column name for title
        let col_name = {
            let state = tab.state.read();
            state
                .csv
                .as_ref()
                .and_then(|csv| csv.headers.get(col_idx).cloned())
                .unwrap_or_else(|| format!("Column {}", col_idx + 1))
        };

        // Handle Escape to close
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            tab.filter_state.close_popup();
            return;
        }

        let mut should_close = false;
        let mut should_apply = false;
        let mut should_clear = false;

        // Show current filter status if one exists
        let current_filter = tab.filter_state.filters.get(&col_idx);
        let has_filter = tab.filter_state.has_filter(col_idx);

        egui::Window::new(format!("Filter: {}", col_name))
            .resizable(false)
            .collapsible(false)
            .default_width(420.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(12.0);

                // Compact active filter badge (if exists) - single line, non-intrusive
                if let Some(filter) = current_filter {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} Current: {} {}",
                                egui_phosphor::regular::FUNNEL_SIMPLE,
                                filter.operator.display_name(),
                                if filter.value.is_empty() {
                                    String::new()
                                } else {
                                    format!("\"{}\"", filter.value)
                                }
                            ))
                            .color(Color32::from_rgb(150, 200, 150)),
                        );
                    });
                    ui.add_space(12.0);
                }

                // Compact form layout - operator and value side by side when space allows
                let needs_value = !matches!(
                    tab.filter_state.selected_operator,
                    FilterOperator::Empty | FilterOperator::NotEmpty
                );

                if needs_value {
                    // Two-column layout for operator and value
                    ui.horizontal(|ui| {
                        // Operator column
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("Operator")
                                    .color(Color32::from_rgb(200, 200, 200)),
                            );
                            ui.add_space(6.0);
                            egui::ComboBox::from_id_salt("filter_operator")
                                .width(180.0)
                                .selected_text(tab.filter_state.selected_operator.display_name())
                                .show_ui(ui, |ui| {
                                    for op in FilterOperator::all() {
                                        ui.selectable_value(
                                            &mut tab.filter_state.selected_operator,
                                            *op,
                                            op.display_name(),
                                        );
                                    }
                                });
                        });

                        ui.add_space(16.0);

                        // Value column
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("Value")
                                    .color(Color32::from_rgb(200, 200, 200)),
                            );
                            ui.add_space(6.0);
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut tab.filter_state.filter_input)
                                    .desired_width(180.0),
                            );

                            // Auto-focus when popup opens
                            if tab.filter_state.focus_input {
                                response.request_focus();
                                tab.filter_state.focus_input = false;
                            }

                            // Apply on Enter
                            if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                                should_apply = true;
                            }
                        });
                    });
                } else {
                    // Single column for operators that don't need values
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("Operator").color(Color32::from_rgb(200, 200, 200)),
                        );
                        ui.add_space(6.0);
                        egui::ComboBox::from_id_salt("filter_operator")
                            .width(380.0)
                            .selected_text(tab.filter_state.selected_operator.display_name())
                            .show_ui(ui, |ui| {
                                for op in FilterOperator::all() {
                                    ui.selectable_value(
                                        &mut tab.filter_state.selected_operator,
                                        *op,
                                        op.display_name(),
                                    );
                                }
                            });
                    });
                }

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(12.0);

                // Action buttons - primary action (Apply) on right, secondary on left
                ui.horizontal(|ui| {
                    // Left side: Clear (if filter exists) and Cancel
                    if has_filter
                        && ui
                            .button(format!("{} Clear", egui_phosphor::regular::TRASH))
                            .clicked()
                    {
                        should_clear = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Primary action on far right
                        if ui
                            .button(format!("{} Apply", egui_phosphor::regular::CHECK))
                            .clicked()
                        {
                            should_apply = true;
                        }
                        // Cancel next to Apply
                        if ui
                            .button(format!("{} Cancel", egui_phosphor::regular::X))
                            .clicked()
                        {
                            should_close = true;
                        }
                    });
                });
                ui.add_space(10.0);
            });

        // Handle actions after UI
        if should_apply {
            let operator = tab.filter_state.selected_operator;
            let value = tab.filter_state.filter_input.clone();
            tab.filter_state.apply_filter(col_idx, operator, value);
            self.mark_filter_changed();
        } else if should_clear {
            tab.filter_state.clear_filter(col_idx);
            tab.filter_state.close_popup();
            self.mark_filter_changed();
        } else if should_close {
            tab.filter_state.close_popup();
        }
    }

    /// Render the Keyboard Shortcuts dialog
    fn render_shortcuts_dialog(&mut self, ctx: &egui::Context) {
        if !self.shortcuts_dialog_open {
            return;
        }

        // Handle Escape to close
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.shortcuts_dialog_open = false;
            return;
        }

        let mut should_close = false;

        egui::Window::new(format!(
            "{} Keyboard Shortcuts",
            egui_phosphor::regular::KEYBOARD
        ))
        .default_size([520.0, 450.0])
        .resizable(true)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.add_space(12.0);

            // Scrollable content area
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(350.0)
                .show(ui, |ui| {
                    // Group shortcuts by category
                    ui.vertical(|ui| {
                        // File & Tabs
                        ui.label(
                            egui::RichText::new("File & Tabs")
                                .strong()
                                .size(14.0)
                                .color(Color32::from_rgb(200, 200, 200)),
                        );
                        ui.add_space(8.0);
                        self.render_shortcut_row(ui, "⌘+T", "New Tab");
                        self.render_shortcut_row(ui, "⌘+W", "Close Active Tab");
                        ui.add_space(16.0);

                        // Search & Navigation
                        ui.label(
                            egui::RichText::new("Search & Navigation")
                                .strong()
                                .size(14.0)
                                .color(Color32::from_rgb(200, 200, 200)),
                        );
                        ui.add_space(8.0);
                        self.render_shortcut_row(ui, "⌘+F", "Open/Focus Search");
                        self.render_shortcut_row(ui, "F3", "Find Next Match");
                        self.render_shortcut_row(ui, "⇧+F3", "Find Previous Match");
                        self.render_shortcut_row(ui, "⌘+L", "Go to Row");
                        ui.add_space(16.0);

                        // Columns
                        ui.label(
                            egui::RichText::new("Columns")
                                .strong()
                                .size(14.0)
                                .color(Color32::from_rgb(200, 200, 200)),
                        );
                        ui.add_space(8.0);
                        self.render_shortcut_row(ui, "⌘+Shift+C", "Column Manager");
                        self.render_shortcut_row(ui, "⌘+Z", "Undo Column Action");
                        self.render_shortcut_row(ui, "⌘+Shift+Z", "Redo Column Action");
                        ui.add_space(16.0);

                        // General
                        ui.label(
                            egui::RichText::new("General")
                                .strong()
                                .size(14.0)
                                .color(Color32::from_rgb(200, 200, 200)),
                        );
                        ui.add_space(8.0);
                        self.render_shortcut_row(ui, "Esc", "Close Dialog/Popup");
                    });
                });

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(12.0);

            // Close button
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(format!("{} Close", egui_phosphor::regular::X))
                        .clicked()
                    {
                        should_close = true;
                    }
                });
            });
            ui.add_space(10.0);
        });

        if should_close {
            self.shortcuts_dialog_open = false;
        }
    }

    /// Render a single shortcut row
    fn render_shortcut_row(&self, ui: &mut egui::Ui, shortcut: &str, description: &str) {
        ui.horizontal(|ui| {
            // Description on the left - fixed width column
            ui.set_width(280.0);
            ui.label(egui::RichText::new(description).color(Color32::from_rgb(220, 220, 220)));

            // Shortcut key badge on the right
            egui::Frame::NONE
                .fill(Color32::from_rgb(35, 35, 40))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(55, 55, 60)))
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::symmetric(10, 5))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(shortcut)
                            .size(12.0)
                            .color(Color32::from_rgb(200, 200, 200)),
                    );
                });
        });
        ui.add_space(6.0);
    }

    /// Render the Column Manager dialog
    fn render_column_manager(&mut self, ctx: &egui::Context) {
        self.ensure_tabs();
        let tab = self.active_tab_mut();

        if !tab.column_state.manager_open {
            return;
        }

        // Handle Escape to close
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            tab.column_state.manager_open = false;
            return;
        }

        // Get headers from state
        let headers = {
            let state = tab.state.read();
            state
                .csv
                .as_ref()
                .map(|c| c.headers.clone())
                .unwrap_or_default()
        };

        if headers.is_empty() {
            tab.column_state.manager_open = false;
            return;
        }

        // Initialize column order if needed
        tab.column_state.init_column_order(headers.len());

        let mut should_close = false;
        let mut move_up: Option<usize> = None;
        let mut move_down: Option<usize> = None;

        egui::Window::new(format!("{} Column Manager", egui_phosphor::regular::SLIDERS))
            .default_size([400.0, 500.0])
            .resizable(true)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(12.0);

                // Action buttons at top
                ui.horizontal(|ui| {
                    if ui.button("Show All").clicked() {
                        tab.column_state.show_all_columns();
                    }
                    if ui.button("Hide All").clicked() {
                        tab.column_state.hide_all_columns();
                    }
                    if ui.button("Reset Order").clicked() {
                        tab.column_state.reset_column_order();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let undo_enabled = !tab.column_state.undo_stack.is_empty();
                        let redo_enabled = !tab.column_state.redo_stack.is_empty();

                        if ui
                            .add_enabled(redo_enabled, egui::Button::new("Redo"))
                            .clicked()
                        {
                            tab.column_state.redo();
                        }
                        if ui
                            .add_enabled(undo_enabled, egui::Button::new("Undo"))
                            .clicked()
                        {
                            tab.column_state.undo();
                        }
                    });
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(12.0);

                // Column list
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(350.0)
                    .show(ui, |ui| {
                        // Show columns in their current display order
                        let column_order = tab.column_state.column_order.clone();
                        let visible_columns = tab.column_state.get_visible_columns();

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
                            let is_hidden = tab.column_state.hidden_columns.contains(&original_idx);
                            let visible_idx = original_to_visible.get(&original_idx);

                            ui.horizontal(|ui| {
                                // Visibility checkbox
                                let mut visible = !is_hidden;
                                if ui.checkbox(&mut visible, "").changed() {
                                    tab.column_state.toggle_column(original_idx);
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
                        tab.column_state.get_visible_columns().len(),
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
                tab.column_state.reorder_visible_columns(idx, idx - 1);
            }
        }
        if let Some(idx) = move_down {
            let visible_count = tab.column_state.get_visible_columns().len();
            if idx < visible_count - 1 {
                tab.column_state.reorder_visible_columns(idx, idx + 1);
            }
        }

        if should_close {
            tab.column_state.manager_open = false;
        }
    }
}

impl eframe::App for FastCsvApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll file loader channel (WASM only)
        #[cfg(target_arch = "wasm32")]
        {
            self.ensure_tabs();
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

        self.ensure_tabs();
        let tab_idx = self.active_tab_index;

        // Check for async filtering results
        {
            let tab = &mut self.tabs[tab_idx];
            if let Some(receiver) = &tab.filter_receiver {
                match receiver.try_recv() {
                    Ok((indices, filters, sort_col, sort_dir, duration)) => {
                        tab.filtered_indices = Some(indices);
                        tab.applied_filters = filters;
                        tab.applied_sort_column = sort_col;
                        tab.applied_sort_direction = sort_dir;
                        tab.filter_duration = Some(duration);
                        tab.filter_receiver = None; // Done receiving
                        tab.is_filtering.store(false, Ordering::Relaxed);
                        tab.row_cache.clear(); // Clear cache for new view
                        ctx.request_repaint(); // Trigger update to show results
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        // Still waiting - keep spinner spinning
                        ctx.request_repaint();
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        // Thread panicked or disconnected
                        tab.filter_receiver = None;
                        tab.is_filtering.store(false, Ordering::Relaxed);
                        ctx.request_repaint();
                    }
                }
            }
        }

        // Handle keyboard shortcuts
        // Collect actions first, then apply them after dropping tab borrow
        let mut actions = Vec::new();
        {
            let tab = &mut self.tabs[tab_idx];
            ctx.input(|i| {
                // Cmd/Ctrl+F to open/focus search (always focus, don't toggle off)
                if i.modifiers.command && i.key_pressed(Key::F) {
                    if !tab.search.visible {
                        tab.search.visible = true;
                    }
                    tab.search.focus_input = true; // Always focus when Cmd+F is pressed
                }
                // Escape to close search
                if i.key_pressed(Key::Escape) && tab.search.visible {
                    tab.search.cancel_flag.store(true, Ordering::SeqCst);
                    tab.search.visible = false;
                    tab.search.history_index = None;
                }
                // F3 or Cmd+G for next/prev match
                if i.key_pressed(Key::F3) || (i.modifiers.command && i.key_pressed(Key::G)) {
                    if i.modifiers.shift {
                        actions.push("prev_match");
                    } else {
                        actions.push("next_match");
                    }
                }
                // Cmd+L to open Go to Row dialog
                if i.modifiers.command && i.key_pressed(Key::L) {
                    tab.go_to_row.open = true;
                    tab.go_to_row.focus_input = true;
                    tab.go_to_row.input.clear();
                }
                // Cmd+Shift+C to open Column Manager
                if i.modifiers.command && i.modifiers.shift && i.key_pressed(Key::C) {
                    tab.column_state.manager_open = true;
                }
                // Cmd+Z for undo column actions
                if i.modifiers.command && i.key_pressed(Key::Z) && !i.modifiers.shift {
                    tab.column_state.undo();
                }
                // Cmd+Shift+Z for redo column actions
                if i.modifiers.command && i.modifiers.shift && i.key_pressed(Key::Z) {
                    tab.column_state.redo();
                }
                // Cmd+T for new tab
                if i.modifiers.command && i.key_pressed(Key::T) {
                    actions.push("new_tab");
                }
                // Cmd+W to close active tab
                if i.modifiers.command && i.key_pressed(Key::W) {
                    actions.push("close_tab");
                }
            });
        }
        // Apply actions after dropping tab borrow
        for action in actions {
            match action {
                "next_match" => self.next_match(),
                "prev_match" => self.prev_match(),
                "new_tab" => self.new_tab(),
                "close_tab" => self.close_active_tab(),
                _ => {}
            }
        }

        // Top panel with menu/toolbar (rendered first, appears at top)
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui: &mut egui::Ui| {
                ui.menu_button("File", |ui: &mut egui::Ui| {
                    if ui
                        .button(format!("{} Open...", egui_phosphor::regular::FOLDER_OPEN))
                        .clicked()
                    {
                        ui.close();
                        self.open_file(ctx);
                    }

                    // Open Recent submenu
                    let recent = self.recent_files.get_recent();
                    if !recent.is_empty() {
                        ui.separator();
                        ui.menu_button(
                            format!("{} Open Recent", egui_phosphor::regular::CLOCK),
                            |ui| {
                                for file in recent.iter().take(10) {
                                    let label = if file.name.len() > 50 {
                                        format!("{}...", &file.name[..47])
                                    } else {
                                        file.name.clone()
                                    };
                                    if ui
                                        .button(format!(
                                            "{} {}",
                                            egui_phosphor::regular::FILE_TEXT,
                                            label
                                        ))
                                        .clicked()
                                    {
                                        ui.close();
                                        self.open_recent_file(&file.path, ctx);
                                    }
                                }
                                if recent.len() > 10 {
                                    ui.separator();
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "... and {} more",
                                            recent.len() - 10
                                        ))
                                        .small()
                                        .color(Color32::GRAY),
                                    );
                                }
                            },
                        );
                    }

                    ui.separator();
                    if ui
                        .button(format!("{} Quit", egui_phosphor::regular::SIGN_OUT))
                        .clicked()
                    {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Edit", |ui: &mut egui::Ui| {
                    self.ensure_tabs();
                    let tab = self.active_tab_mut();
                    if ui.button("Find... (⌘+F)").clicked() {
                        ui.close();
                        tab.search.visible = true;
                        tab.search.focus_input = true;
                    }
                    let has_nav_rows = !tab.search.results.read().navigation_rows.is_empty();
                    let should_next = ui
                        .add_enabled(has_nav_rows, egui::Button::new("Find Next (F3)"))
                        .clicked();
                    let should_prev = ui
                        .add_enabled(has_nav_rows, egui::Button::new("Find Previous (⇧F3)"))
                        .clicked();
                    let should_go_to_row = ui.button("Go to Row... (⌘+L)").clicked();
                    // Release tab borrow before calling methods
                    if should_next {
                        let _ = tab; // Release borrow
                        ui.close();
                        self.next_match();
                    } else if should_prev {
                        let _ = tab; // Release borrow
                        ui.close();
                        self.prev_match();
                    } else if should_go_to_row {
                        ui.close();
                        tab.go_to_row.open = true;
                        tab.go_to_row.focus_input = true;
                        tab.go_to_row.input.clear();
                    }
                    ui.separator();
                });
                ui.menu_button("View", |ui: &mut egui::Ui| {
                    self.ensure_tabs();
                    let tab = self.active_tab_mut();
                    if ui.button("Column Manager... (⌘+Shift+C)").clicked() {
                        ui.close();
                        tab.column_state.manager_open = true;
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
                ui.menu_button("Help", |ui: &mut egui::Ui| {
                    if ui
                        .button(format!(
                            "{} Keyboard Shortcuts",
                            egui_phosphor::regular::KEYBOARD
                        ))
                        .clicked()
                    {
                        ui.close();
                        self.shortcuts_dialog_open = true;
                    }
                });
            });
        });

        // Tab bar panel (rendered after menu, appears below menu bar)
        egui::TopBottomPanel::top("tab_bar")
            .exact_height(26.0)
            .show(ctx, |ui| {
                self.render_tab_bar(ui);
            });

        // Search bar panel (shown below menu when search is active)
        self.ensure_tabs();
        let search_visible = {
            let tab = &self.tabs[self.active_tab_index];
            tab.search.visible
        };
        if search_visible {
            egui::TopBottomPanel::top("search_panel")
                .exact_height(36.0)
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    self.render_search_bar(ui, ctx);
                });
        }

        // Update available banner
        let update_available = self.update_state.update_available.load(Ordering::Relaxed);
        let update_dismissed = self.update_state.dismissed;
        if update_available && !update_dismissed {
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
            self.ensure_tabs();
            // Get load_state first without holding tab borrow
            let load_state = {
                let tab = &self.tabs[self.active_tab_index];
                let state = tab.state.read();
                state.load_state
            };

            // Expand window when file is loaded (only once, native only)
            #[cfg(not(target_arch = "wasm32"))]
            {
                if matches!(load_state, LoadState::Ready) && !self.window_expanded {
                    self.window_expanded = true;
                    let new_size = [1200.0, 800.0];
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(new_size.into()));

                    // Center the window on the screen
                    // Get the current viewport position and size to calculate center
                    let viewport = ctx.input(|i| i.viewport().clone());
                    if let Some(outer_rect) = viewport.outer_rect {
                        let current_pos = outer_rect.min;
                        let current_size = outer_rect.size();

                        // Calculate offset to keep window centered relative to its current position
                        let offset_x = (new_size[0] - current_size.x) / 2.0;
                        let offset_y = (new_size[1] - current_size.y) / 2.0;
                        let new_pos = [
                            current_pos.x - offset_x,
                            current_pos.y - offset_y,
                        ];
                        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(new_pos.into()));
                    }
                }
            }

            match load_state {
                LoadState::Empty => {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.heading("QuickCSV");
                            ui.add_space(16.0);

                            // Open file button with icon
                            if ui
                                .button(format!(
                                    "{} Open CSV File",
                                    egui_phosphor::regular::FOLDER_OPEN
                                ))
                                .clicked()
                            {
                                self.open_file(ctx);
                            }
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("or drag and drop a file here")
                                    .color(Color32::GRAY)
                                    .small(),
                            );

                            // Show recent files if available
                            let recent = self.recent_files.get_recent();
                            if !recent.is_empty() {
                                ui.add_space(24.0);
                                ui.separator();
                                ui.add_space(12.0);

                                // Recent Files header
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} Recent Files",
                                            egui_phosphor::regular::CLOCK
                                        ))
                                        .size(14.0)
                                        .strong(),
                                    );
                                });
                                ui.add_space(8.0);

                                // Recent files list with better styling
                                egui::Frame::NONE
                                    .fill(ui.style().visuals.panel_fill)
                                    .inner_margin(6.0)
                                    .show(ui, |ui| {
                                        egui::ScrollArea::vertical()
                                            .max_height(200.0)
                                            .show(ui, |ui| {
                                                for file in recent.iter().take(8) {
                                                    ui.horizontal(|ui| {
                                                        // File icon and name button
                                                        let label = if file.name.len() > 40 {
                                                            format!("{}...", &file.name[..37])
                                                        } else {
                                                            file.name.clone()
                                                        };

                                                        let button_text = format!(
                                                            "{} {}",
                                                            egui_phosphor::regular::FILE_TEXT,
                                                            label
                                                        );

                                                        let response = ui
                                                            .button(button_text)
                                                            .on_hover_text(&file.path);

                                                        if response.clicked() {
                                                            self.open_recent_file(&file.path, ctx);
                                                        }

                                                        ui.with_layout(
                                                            egui::Layout::right_to_left(egui::Align::Center),
                                                            |ui| {
                                                                #[cfg(not(target_arch = "wasm32"))]
                                                                {
                                                                    if ui
                                                                        .small_button(
                                                                            egui_phosphor::regular::X,
                                                                        )
                                                                        .on_hover_text("Remove from recent files")
                                                                        .clicked()
                                                                    {
                                                                        self.recent_files
                                                                            .remove_file(&file.path);
                                                                        self.save_recent_files();
                                                                    }
                                                                }
                                                                #[cfg(target_arch = "wasm32")]
                                                                {
                                                                    let _ = ui; // Suppress unused warning
                                                                }
                                                            },
                                                        );
                                                    });
                                                    ui.add_space(2.0);
                                                }
                                            });
                                    });
                            }
                        });
                    });
                }
                LoadState::Indexing => {
                    // Get row count without holding tab borrow
                    let rows = {
                        let tab = &self.tabs[tab_idx];
                        let state = tab.state.read();
                        state.rows_indexed.load(Ordering::Relaxed)
                    };
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.spinner();
                            ui.add_space(10.0);
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
                    // Get error message without holding tab borrow
                    let error_msg = {
                        let tab = &self.tabs[tab_idx];
                        let state = tab.state.read();
                        state.error_message.clone()
                    };
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.colored_label(
                                egui::Color32::RED,
                                format!(
                                    "Error: {}",
                                    error_msg.as_deref().unwrap_or("Unknown")
                                ),
                            );
                            ui.add_space(20.0);
                            if ui.button("Try Again").clicked() {
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

        // Render Keyboard Shortcuts dialog (if open)
        self.render_shortcuts_dialog(ctx);

        // Handle dropped files
        #[cfg(target_arch = "wasm32")]
        {
            let dropped_file: Option<(String, Vec<u8>)> = ctx.input(|i| {
                if !i.raw.dropped_files.is_empty() {
                    let file = &i.raw.dropped_files[0];
                    file.bytes
                        .as_ref()
                        .map(|bytes| (file.name.clone(), bytes.to_vec()))
                } else {
                    None
                }
            });
            if let Some((name, bytes)) = dropped_file {
                // Add to recent files before loading
                self.recent_files.add_file(name.clone(), name.clone());
                self.save_recent_files();
                self.load_file_web(name, bytes, ctx.clone());
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            // Collect dropped file paths first to avoid borrow conflicts
            let mut dropped_paths = Vec::new();
            ctx.input(|i| {
                for file in &i.raw.dropped_files {
                    if let Some(path) = &file.path {
                        dropped_paths.push(path.clone());
                    }
                }
            });

            // Open each dropped file in a new tab (first file is enough for now)
            if let Some(path) = dropped_paths.into_iter().next() {
                let path_str = path.to_string_lossy().to_string();
                let file_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&path_str)
                    .to_string();
                self.recent_files.add_file(path_str, file_name);
                self.save_recent_files();

                // Create a new tab for the dropped file
                let new_tab = TabState::from_path(path.clone());
                self.tabs.push(new_tab);
                self.active_tab_index = self.tabs.len() - 1;
                self.load_file(path, ctx.clone());
            }
        }
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
            .with_inner_size([700.0, 500.0])
            .with_min_inner_size([500.0, 350.0])
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

            // Load recent files from storage
            let mut app = FastCsvApp::default();
            app.load_recent_files();
            app.recent_files_loaded = true;

            Ok(Box::new(app))
        }),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    // Redirect panic to console log
    console_error_panic_hook::set_once();

    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        let window = web_sys::window().expect("No window");
        let document = window.document().expect("No document");

        // Prevent browser's default Cmd+F/Ctrl+F behavior
        // Use bubble phase (not capture) so egui gets the event first, then we prevent browser default
        {
            let doc_closure = wasm_bindgen::closure::Closure::wrap(Box::new(
                move |event: web_sys::KeyboardEvent| {
                    // Check for Cmd+F (Mac) or Ctrl+F (Windows/Linux)
                    let is_cmd_or_ctrl = event.meta_key() || event.ctrl_key();
                    let is_f = event.key() == "f" || event.key() == "F" || event.code() == "KeyF";

                    if is_cmd_or_ctrl && is_f {
                        // Prevent browser's default find dialog
                        // This runs in bubble phase, so egui should have already handled it
                        event.prevent_default();
                    }
                },
            )
                as Box<dyn FnMut(_)>);

            // Add listener in bubble phase (default, useCapture = false)
            // This allows egui to handle the event first, then we prevent browser default
            document
                .add_event_listener_with_callback("keydown", doc_closure.as_ref().unchecked_ref())
                .expect("Failed to add keydown listener to document");

            // Keep closure alive
            doc_closure.forget();
        }

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

                    // Load recent files from storage
                    let mut app = FastCsvApp::default();
                    app.load_recent_files();
                    app.recent_files_loaded = true;

                    Ok(Box::new(app))
                }),
            )
            .await
            .expect("failed to start eframe");
    });
}

#[cfg(test)]
mod search_tests {
    use crate::state::search::SearchResults;
    use crate::state::{SearchState, SearchStatus};

    /// Test that search query matching works correctly
    #[test]
    fn test_search_query_matching() {
        // Test basic case-insensitive matching
        let query = "test";
        let field1 = "This is a TEST string";
        let field2 = "No match here";
        let field3 = "test";

        assert!(field1.to_lowercase().contains(&query));
        assert!(!field2.to_lowercase().contains(&query));
        assert!(field3.to_lowercase().contains(&query));
    }

    /// Test that empty query doesn't trigger search
    #[test]
    fn test_empty_query() {
        let query = "   ".trim().to_lowercase();
        assert!(query.is_empty());

        let query2 = "".trim().to_lowercase();
        assert!(query2.is_empty());
    }

    /// Test search state initialization
    #[test]
    fn test_search_state_default() {
        let search_state = SearchState::default();
        assert!(!search_state.visible);
        assert!(search_state.query.is_empty());
        assert!(search_state.active_query.is_empty());
        assert_eq!(search_state.current_index, 0);
        assert_eq!(search_state.results.read().status, SearchStatus::Idle);
        assert_eq!(search_state.results.read().total_match_count, 0);
        assert!(search_state.results.read().navigation_rows.is_empty());
    }

    /// Test search results structure
    #[test]
    fn test_search_results_default() {
        use crate::state::search::SearchResults;
        let results = SearchResults::default();
        assert_eq!(results.status, SearchStatus::Idle);
        assert_eq!(results.rows_searched, 0);
        assert_eq!(results.total_rows, 0);
        assert_eq!(results.total_match_count, 0);
        assert!(results.navigation_rows.is_empty());
        assert!(!results.nav_limit_reached);
    }

    /// Test that search query trimming works
    #[test]
    fn test_query_trimming() {
        let queries = vec![
            ("  test  ", "test"),
            ("test", "test"),
            ("  test", "test"),
            ("test  ", "test"),
            ("", ""),
        ];

        for (input, expected) in queries {
            let trimmed = input.trim().to_lowercase();
            assert_eq!(
                trimmed,
                expected.to_lowercase(),
                "Failed for input: {:?}",
                input
            );
        }
    }

    /// Test search status transitions
    #[test]
    fn test_search_status() {
        let status_idle = SearchStatus::Idle;
        let status_searching = SearchStatus::Searching;
        let status_complete = SearchStatus::Complete;
        let status_cancelled = SearchStatus::Cancelled;

        // Test that statuses are distinct
        assert_ne!(status_idle, status_searching);
        assert_ne!(status_searching, status_complete);
        assert_ne!(status_complete, status_cancelled);
    }

    /// Test that search query case insensitivity
    #[test]
    fn test_case_insensitive_search() {
        let query = "HELLO";
        let test_cases = vec![
            ("hello", true),
            ("HELLO", true),
            ("Hello", true),
            ("HeLlO", true),
            ("world", false),
            ("hell", false),
            ("hello world", true),
            ("say hello", true),
        ];

        let query_lower = query.to_lowercase();
        for (text, should_match) in test_cases {
            let matches = text.to_lowercase().contains(&query_lower);
            assert_eq!(matches, should_match, "Failed for text: {:?}", text);
        }
    }
}
