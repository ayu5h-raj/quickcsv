//! QuickCSV - High-Performance CSV Viewer for macOS
//!
//! A memory-mapped, virtualized CSV viewer that can handle files from 100MB to 2GB+
//! with zero lag. Uses memmap2 for zero-copy file loading and egui for the UI.

// Module declarations
pub mod csv;
pub mod state;
pub mod update;
pub mod utils;

use eframe::egui::{self, Color32, Key};

use parking_lot::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_arch = "wasm32")]
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::thread;

#[cfg(target_arch = "wasm32")]
async fn yield_to_browser() {
    // Use setTimeout(0) to yield to the macro-task queue, allowing UI rendering
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let window = web_sys::window().expect("should have a window");
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

// Imports from modules
#[cfg(not(target_arch = "wasm32"))]
use csv::init_csv_progressive;
use state::{
    GoToRowState, JsonViewerState, LoadState, RowDetailState, SearchState, SearchStatus,
    SharedState, SortDirection, SortState, MAX_NAV_ROWS,
};
#[cfg(not(target_arch = "wasm32"))]
use update::check_for_updates;
use update::UpdateState;
use utils::{format_file_size, format_json, format_number, looks_like_json, truncate_for_display};

/// Height of each row in pixels
const ROW_HEIGHT: f32 = 24.0;

/// Highlight color for search matches
const HIGHLIGHT_COLOR: Color32 = Color32::from_rgb(255, 200, 0);

/// Highlight color for current/active search match
const CURRENT_MATCH_COLOR: Color32 = Color32::from_rgb(255, 120, 0);

/// Default column width
const DEFAULT_COLUMN_WIDTH: f32 = 150.0;

/// Main application state
pub struct FastCsvApp {
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
    #[cfg(target_arch = "wasm32")]
    /// Channel for receiving loaded file data from async web tasks
    file_loader_tx: Sender<(String, Vec<u8>)>,
    #[cfg(target_arch = "wasm32")]
    file_loader_rx: Receiver<(String, Vec<u8>)>,
}

impl Default for FastCsvApp {
    fn default() -> Self {
        #[cfg(target_arch = "wasm32")]
        let (tx, rx) = channel();

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
            #[cfg(target_arch = "wasm32")]
            file_loader_tx: tx,
            #[cfg(target_arch = "wasm32")]
            file_loader_rx: rx,
        }
    }
}

impl FastCsvApp {
    /// Open a file dialog and load the selected CSV file
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

    #[cfg(target_arch = "wasm32")]
    fn open_file_web(&mut self, ctx: egui::Context) {
        let task = rfd::AsyncFileDialog::new()
            .add_filter("CSV", &["csv", "tsv", "txt"])
            .pick_file();

        let tx = self.file_loader_tx.clone();
        let state = self.state.clone();
        let ctx = ctx.clone();

        // Indicate loading started (spinner will show if we set state here)
        // But better to wait until we actually receive data to avoid premature clearing

        wasm_bindgen_futures::spawn_local(async move {
            if let Some(file) = task.await {
                let name: String = file.file_name();
                let bytes = file.read().await;

                // Show loading state immediately to give feedback
                {
                    let mut state = state.write();
                    state.load_state = LoadState::Indexing;
                }
                ctx.request_repaint();

                // Send to main thread for processing
                if let Err(e) = tx.send((name, bytes)) {
                    web_sys::console::error_1(&format!("Failed to send file data: {}", e).into());
                }

                // Trigger repaint to process the channel
                ctx.request_repaint();
            }
        });
    }

    /// Load a CSV file in the background
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

    /// Sort data by column
    ///
    /// Clicking the same column cycles through: None -> Ascending -> Descending -> None
    /// Clicking a different column starts with Ascending
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

        // Spawn background sorting thread (Native)
        #[cfg(not(target_arch = "wasm32"))]
        let sort_task = move || {
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

            // Collect row data
            let mut row_data: Vec<(usize, String)> = Vec::with_capacity(rows_to_sort);
            for row_idx in 0..rows_to_sort {
                if cancel_flag.load(Ordering::Relaxed) {
                    is_sorting.store(false, Ordering::SeqCst);
                    return;
                }

                if let Some(fields) = csv.parse_row(row_idx) {
                    let value = fields.get(col_idx).cloned().unwrap_or_default();
                    row_data.push((row_idx, value));
                }

                if row_idx % 5000 == 0 {
                    let pct = (row_idx * 50) / rows_to_sort;
                    progress.store(pct, Ordering::Relaxed);
                    ctx.request_repaint();
                }
            }

            progress.store(50, Ordering::Relaxed);
            ctx.request_repaint();

            // Sort
            match new_direction {
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

            {
                let mut indices = sorted_indices.write();
                *indices = row_data.into_iter().map(|(idx, _)| idx).collect();
            }

            progress.store(100, Ordering::Relaxed);
            is_sorting.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        };

        #[cfg(not(target_arch = "wasm32"))]
        thread::spawn(sort_task);

        #[cfg(target_arch = "wasm32")]
        {
            let direction = new_direction;
            wasm_bindgen_futures::spawn_local(async move {
                // Async collection phase
                const MAX_SORT_ROWS: usize = 100_000;

                let (_total_rows, rows_to_sort) = {
                    let state_guard = state.read();
                    if let Some(csv) = &state_guard.csv {
                        let total = csv.indexed_row_count();
                        (total, total.min(MAX_SORT_ROWS))
                    } else {
                        is_sorting.store(false, Ordering::SeqCst);
                        return;
                    }
                };

                let mut row_data: Vec<(usize, String)> = Vec::with_capacity(rows_to_sort);
                const BATCH_SIZE: usize = 5000;

                for start_idx in (0..rows_to_sort).step_by(BATCH_SIZE) {
                    if cancel_flag.load(Ordering::Relaxed) {
                        is_sorting.store(false, Ordering::SeqCst);
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
                // If this is too slow, we'd need an async sort algo.
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
                is_sorting.store(false, Ordering::SeqCst);
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

    /// Execute search across all columns in a background thread
    ///
    /// This search is optimized for large files:
    /// - Counts ALL matches (no limit) for accurate totals
    /// - Only stores limited navigation rows (for prev/next)
    /// - Highlighting is done on-the-fly during rendering
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

        // Get total rows for progress
        let total_rows = {
            let state = self.state.read();
            state
                .csv
                .as_ref()
                .map(|c| c.indexed_row_count())
                .unwrap_or(0)
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

        // Spawn background search thread
        #[cfg(not(target_arch = "wasm32"))]
        let search_task = move || {
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

                // Parse and search the row
                if let Some(fields) = csv.parse_row(row_idx) {
                    let mut row_has_match = false;
                    for field in fields.iter() {
                        if field.to_lowercase().contains(&query) {
                            total_matches += 1;
                            row_has_match = true;
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
            results.status = SearchStatus::Complete;
            results.nav_limit_reached = nav_limit_reached;
            drop(results);
            ctx.request_repaint();
        };

        #[cfg(not(target_arch = "wasm32"))]
        thread::spawn(search_task);

        #[cfg(target_arch = "wasm32")]
        {
            // For Web, we must run asynchronously and yield to the browser
            // to prevent freezing the UI during long searches.
            // We replicate the logic from search_task but add yields.

            let query = self.search.active_query.clone().trim().to_lowercase();
            // let cancel_flag = Arc::clone(&self.search.cancel_flag); // Use outer variable

            wasm_bindgen_futures::spawn_local(async move {
                const BATCH_SIZE: usize = 1000;
                let mut row_idx = 0;
                let mut nav_rows: Vec<usize> = Vec::new();
                let mut total_matches: usize = 0;
                let mut nav_limit_reached = false;
                const MAX_NAV_ROWS: usize = 1000; // Limit navigation rows

                loop {
                    // 1. Process a batch of rows under read lock
                    let (should_continue, rows_processed, chunk_matches, limit_reached) = {
                        let state_guard = state.read();
                        let csv = match &state_guard.csv {
                            Some(csv) => csv,
                            None => {
                                let mut results = results.write();
                                results.status = SearchStatus::Idle;
                                return;
                            }
                        };

                        let total_rows = csv.indexed_row_count();
                        if row_idx >= total_rows {
                            (false, 0, vec![], false)
                        } else {
                            let end_idx = (row_idx + BATCH_SIZE).min(total_rows);
                            let mut matches = Vec::new();
                            let mut batch_limit_reached = false;

                            for i in row_idx..end_idx {
                                // Check cancellation inside batch? Maybe too granular for WASM,
                                // check once per batch is fine.
                                if cancel_flag.load(Ordering::Relaxed) {
                                    return; // Handled outside loop
                                }

                                if let Some(fields) = csv.parse_row(i) {
                                    let mut row_has_match = false;
                                    for field in fields.iter() {
                                        if field.to_lowercase().contains(&query) {
                                            row_has_match = true;
                                            break;
                                        }
                                    }

                                    if row_has_match {
                                        matches.push(i);
                                        // Check limit
                                        // Note: We only count matches for nav rows up to limit,
                                        // but ideally we want TOTAL match count.
                                        // Replicating search_task logic:
                                        // Total matches always increments.
                                        // Nav rows only appends if < limit.
                                        if nav_rows.len() + matches.len() > MAX_NAV_ROWS {
                                            batch_limit_reached = true;
                                        }
                                    }
                                }
                            }
                            (true, end_idx - row_idx, matches, batch_limit_reached)
                        }
                    };

                    // Check cancellation again (if we returned early)
                    if cancel_flag.load(Ordering::Relaxed) {
                        let mut results = results.write();
                        results.status = SearchStatus::Cancelled;
                        ctx.request_repaint();
                        return;
                    }

                    if !should_continue {
                        break;
                    }

                    // 2. Update results with this batch
                    total_matches += chunk_matches.len();
                    if !nav_limit_reached {
                        if limit_reached {
                            nav_limit_reached = true;
                            // Add remaining space
                            let space = MAX_NAV_ROWS.saturating_sub(nav_rows.len());
                            nav_rows.extend(chunk_matches.into_iter().take(space));
                        } else {
                            // Check if adding this chunk exceeds limit
                            if nav_rows.len() + chunk_matches.len() > MAX_NAV_ROWS {
                                nav_limit_reached = true;
                                let space = MAX_NAV_ROWS - nav_rows.len();
                                nav_rows.extend(chunk_matches.into_iter().take(space));
                            } else {
                                nav_rows.extend(chunk_matches);
                            }
                        }
                    } else {
                        // already reached limit, just counting total_matches
                    }

                    // Update UI progress
                    if row_idx % 10000 == 0 || !should_continue {
                        let mut results = results.write();
                        // We don't update navigation_rows incrementally to avoid UI flicker/resorts?
                        // Actually search_task doesn't either. It does it at end.
                        // But we want progress.
                        results.rows_searched = row_idx + rows_processed;
                        results.total_match_count = total_matches;
                        ctx.request_repaint();
                    }

                    row_idx += rows_processed;

                    // 3. Yield to browser to let UI render
                    yield_to_browser().await;
                }

                // Final update
                let mut results = results.write();
                results.navigation_rows = nav_rows;
                results.total_match_count = total_matches;
                results.rows_searched = row_idx; // Should be total
                results.status = SearchStatus::Complete;
                results.nav_limit_reached = nav_limit_reached;
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
                    .hint_text("Search all columns...")
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

        // Ensure we have column widths
        if self.column_widths.len() != num_columns {
            self.column_widths = vec![DEFAULT_COLUMN_WIDTH; num_columns];
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

                // Add data columns with initial width - use clip(true) to prevent overflow
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

                        // Data column headers
                        for (col_idx, col_name) in headers.iter().enumerate() {
                            header.col(|ui| {
                                // Create clickable header with sort indicator
                                let sort_indicator = if current_sort_col == Some(col_idx) {
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
                                let text = if is_sorting && current_sort_col == Some(col_idx) {
                                    egui::RichText::new(header_text)
                                        .strong()
                                        .color(Color32::from_rgb(100, 180, 255))
                                } else {
                                    egui::RichText::new(header_text).strong()
                                };

                                let response =
                                    ui.add(egui::Label::new(text).sense(egui::Sense::click()));

                                // Only allow clicks when not sorting
                                if response.clicked() && !is_sorting {
                                    clicked_column = Some(col_idx);
                                }

                                // Show tooltip
                                if is_sorting {
                                    response.on_hover_text("Sorting in progress...");
                                } else {
                                    response.on_hover_text("Click to sort");
                                }
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(ROW_HEIGHT, total_rows, |mut row| {
                            let display_idx = row.index();

                            // Map display index to actual row index (for sorting)
                            let actual_row_idx = self.get_actual_row_index(display_idx);

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

                            // Render each cell with ON-THE-FLY search highlighting
                            // This is O(visible_rows) per frame - no memory overhead!
                            for (col_idx, field) in fields.iter().enumerate() {
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
                                        // Get column name
                                        let col_name = headers
                                            .get(col_idx)
                                            .cloned()
                                            .unwrap_or_else(|| format!("Column {col_idx}"));

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
                                        self.json_viewer.col = col_idx;
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

                            // Fill remaining columns if row has fewer fields than headers
                            for _ in fields.len()..num_columns {
                                row.col(|ui| {
                                    ui.label("");
                                });
                            }
                        });
                    });

                // Handle column header click for sorting (after table is built)
                if let Some(col_idx) = clicked_column {
                    self.sort_by_column(col_idx, ui.ctx());
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

                    if is_still_indexing {
                        ui.spinner();
                        ui.label(format!("Rows: {}...", format_number(row_count)));
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
                        ui.ctx().copy_text(formatted.clone());
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
                        ui.ctx().copy_text(raw.clone());
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
                            ui.ctx().copy_text(csv_row);
                        }

                        if ui
                            .button(format!("{} Copy as JSON", egui_phosphor::regular::CODE))
                            .clicked()
                        {
                            let mut json_obj = serde_json::Map::new();
                            for (i, field) in self.row_detail.fields.iter().enumerate() {
                                let header = self
                                    .row_detail
                                    .headers
                                    .get(i)
                                    .cloned()
                                    .unwrap_or_else(|| format!("col_{}", i));
                                json_obj.insert(header, serde_json::Value::String(field.clone()));
                            }
                            if let Ok(json_str) = serde_json::to_string_pretty(&json_obj) {
                                ui.ctx().copy_text(json_str);
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
                                                    ui.ctx().copy_text(field.clone());
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
    #[cfg(target_arch = "wasm32")]
    fn load_file_web(&mut self, name: String, bytes: Vec<u8>, ctx: egui::Context) {
        // Reset state
        self.scroll_y = 0.0;
        self.scroll_x = 0.0;
        self.column_widths.clear();
        self.row_cache.clear();
        self.last_visible_range = (0, 0);
        self.sort_state = SortState::default();
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

        use csv::init_csv_web;
        let state = Arc::clone(&self.state);
        let ctx_clone = ctx.clone();

        // Run synchronously for now
        let result = init_csv_web(name, bytes, &state, &ctx_clone);

        if let Err(e) = result {
            let mut state_guard = state.write();
            state_guard.load_state = LoadState::Error;
            state_guard.error_message = Some(e);
            ctx.request_repaint();
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
        });

        // Top panel with menu/toolbar
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui: &mut egui::Ui| {
                ui.menu_button("File", |ui: &mut egui::Ui| {
                    if ui.button("Open...").clicked() {
                        ui.close();
                        #[cfg(not(target_arch = "wasm32"))]
                        self.open_file(ctx);
                        #[cfg(target_arch = "wasm32")]
                        self.open_file_web(ctx.clone());
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Edit", |ui: &mut egui::Ui| {
                    if ui.button("Find... (⌘F)").clicked() {
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
                    if ui.button("Go to Row... (⌘L)").clicked() {
                        ui.close();
                        self.go_to_row.open = true;
                        self.go_to_row.focus_input = true;
                        self.go_to_row.input.clear();
                    }
                });
                ui.menu_button("View", |ui: &mut egui::Ui| {
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
        #[cfg(not(target_arch = "wasm32"))]
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
                                    let script = "tell application \"Terminal\" to do script \"brew update && brew upgrade --cask quickcsv && open -a QuickCSV\"";

                                    let success = std::process::Command::new("osascript")
                                        .arg("-e")
                                        .arg(script)
                                        .spawn()
                                        .is_ok();

                                    // Fallback: copy command to clipboard if Terminal automation fails
                                    if !success {
                                        if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                            let _ = clipboard.set_text("brew update && brew upgrade --cask quickcsv");
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
                                #[cfg(not(target_arch = "wasm32"))]
                                self.open_file(ctx);
                                #[cfg(target_arch = "wasm32")]
                                self.open_file_web(ctx.clone());
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
                                #[cfg(not(target_arch = "wasm32"))]
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

        // Handle dropped files
        #[cfg(not(target_arch = "wasm32"))]
        ctx.input(|i| {
            for file in &i.raw.dropped_files {
                if let Some(path) = &file.path {
                    self.load_file(path.clone(), ctx.clone());
                    break;
                }
            }
        });

        // Handle dropped files (Web)
        #[cfg(target_arch = "wasm32")]
        ctx.input(|i| {
            if !i.raw.dropped_files.is_empty() {
                let file = &i.raw.dropped_files[0];
                if let Some(bytes) = &file.bytes {
                    self.load_file_web(file.name.clone(), bytes.to_vec(), ctx.clone());
                }
            }
        });
    }
}

// ----------------------------------------------------------------------------
// Web Entry Point
// ----------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
use eframe::wasm_bindgen::{self, prelude::*};

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub async fn start() -> Result<(), wasm_bindgen::JsValue> {
    // Redirect panic messages to console.log
    std::panic::set_hook(Box::new(console_error_panic_hook::hook));

    web_sys::console::log_1(&"QuickCSV: Starting WASM initialization...".into());

    let web_options = eframe::WebOptions::default();

    let window = web_sys::window().ok_or_else(|| {
        let err = "Failed to get window object".into();
        web_sys::console::error_1(&err);
        err
    })?;

    let document = window.document().ok_or_else(|| {
        let err = "Failed to get document object".into();
        web_sys::console::error_1(&err);
        err
    })?;

    // Remove loading text
    if let Some(loading_text) = document.get_element_by_id("center_text") {
        loading_text.remove();
        web_sys::console::log_1(&"QuickCSV: Removed loading text".into());
    }

    let canvas = document
        .get_element_by_id("the_canvas_id")
        .ok_or_else(|| {
            let err = "Failed to find canvas with id 'the_canvas_id'".into();
            web_sys::console::error_1(&err);
            err
        })?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|e| {
            let err = format!("Failed to cast to HtmlCanvasElement: {:?}", e).into();
            web_sys::console::error_1(&err);
            err
        })?;

    web_sys::console::log_1(&"QuickCSV: Starting WebRunner...".into());

    eframe::WebRunner::new()
        .start(
            canvas,
            web_options,
            Box::new(|cc| {
                web_sys::console::log_1(&"QuickCSV: Creating app instance...".into());
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
        .map_err(|e| {
            let err = format!("WebRunner failed: {:?}", e).into();
            web_sys::console::error_1(&err);
            err
        })?;

    web_sys::console::log_1(&"QuickCSV: Initialization complete!".into());
    Ok(())
}
