//! QuickCSV - High-Performance CSV Viewer for macOS
//!
//! A memory-mapped, virtualized CSV viewer that can handle files from 100MB to 2GB+
//! with zero lag. Uses memmap2 for zero-copy file loading and egui for the UI.

use eframe::egui::{self, Color32, Key};
use memmap2::Mmap;
use parking_lot::RwLock;
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

/// Height of each row in pixels
const ROW_HEIGHT: f32 = 24.0;

/// Highlight color for search matches
const HIGHLIGHT_COLOR: Color32 = Color32::from_rgb(255, 200, 0);

/// Highlight color for current/active search match
const CURRENT_MATCH_COLOR: Color32 = Color32::from_rgb(255, 120, 0);

/// Default column width
const DEFAULT_COLUMN_WIDTH: f32 = 150.0;

/// State of CSV file loading and indexing
#[derive(Clone, Copy, PartialEq)]
enum LoadState {
    Empty,
    Indexing,
    Ready,
    Error,
}

/// Memory-mapped CSV file with row offset index
struct MappedCsv {
    /// Memory-mapped file data
    mmap: Mmap,
    /// Byte offset of each row (including header)
    row_offsets: Vec<usize>,
    /// Column headers
    headers: Vec<String>,
    /// Total number of data rows (excluding header)
    total_rows: usize,
    /// File path
    path: PathBuf,
    /// File size in bytes
    file_size: u64,
}

impl MappedCsv {
    /// Get a row's data as a slice of the memory-mapped region
    fn get_row_bytes(&self, row_index: usize) -> Option<&[u8]> {
        if row_index >= self.total_rows {
            return None;
        }
        // Row 0 is the header, so data rows start at index 1
        let data_row_offset_index = row_index + 1;
        if data_row_offset_index >= self.row_offsets.len() {
            return None;
        }

        let start = self.row_offsets[data_row_offset_index];
        let end = if data_row_offset_index + 1 < self.row_offsets.len() {
            self.row_offsets[data_row_offset_index + 1]
        } else {
            self.mmap.len()
        };

        Some(&self.mmap[start..end])
    }

    /// Parse a single row into fields
    fn parse_row(&self, row_index: usize) -> Option<Vec<String>> {
        let bytes = self.get_row_bytes(row_index)?;
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(bytes);

        reader.records().next().and_then(|result| {
            result
                .ok()
                .map(|record| record.iter().map(|s| s.to_string()).collect())
        })
    }
}

/// Shared state between UI thread and background indexer
struct SharedState {
    /// The loaded CSV file (None if no file loaded)
    csv: Option<MappedCsv>,
    /// Current loading state
    load_state: LoadState,
    /// Error message if loading failed
    error_message: Option<String>,
    /// Number of rows indexed so far (for progress display)
    rows_indexed: AtomicUsize,
    /// Flag to cancel ongoing indexing
    cancel_indexing: AtomicBool,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            csv: None,
            load_state: LoadState::Empty,
            error_message: None,
            rows_indexed: AtomicUsize::new(0),
            cancel_indexing: AtomicBool::new(false),
        }
    }
}

/// Search execution state
#[derive(Clone, Copy, PartialEq, Default)]
enum SearchStatus {
    #[default]
    Idle,
    Searching,
    Complete,
    Cancelled,
}

/// Shared search results (updated by background thread)
///
/// Design: We DON'T store all matches. Instead:
/// - Count total matches for display
/// - Store limited navigation rows (first N rows with matches)
/// - Highlighting is done on-the-fly during rendering (viewport only)
struct SearchResults {
    /// Current search status
    status: SearchStatus,
    /// Number of rows searched so far
    rows_searched: usize,
    /// Total rows to search
    total_rows: usize,
    /// Total number of matching cells found (for display only)
    total_match_count: usize,
    /// Row indices that contain matches (limited for navigation)
    /// Only stores first MAX_NAV_ROWS unique rows
    navigation_rows: Vec<usize>,
    /// Whether we've hit the navigation row limit
    nav_limit_reached: bool,
}

/// Maximum rows to store for navigation (keeps memory bounded)
const MAX_NAV_ROWS: usize = 10000;

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
struct SearchState {
    /// Whether search bar is visible
    visible: bool,
    /// Current search query (in the text field)
    query: String,
    /// The active/confirmed search query (lowercase, for matching)
    active_query: String,
    /// Current navigation index (for prev/next through rows)
    current_index: usize,
    /// Row to scroll to (set when navigating matches)
    scroll_to_row: Option<usize>,
    /// Shared search results (updated by background thread)
    results: Arc<RwLock<SearchResults>>,
    /// Flag to cancel ongoing search
    cancel_flag: Arc<AtomicBool>,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            visible: false,
            query: String::new(),
            active_query: String::new(),
            current_index: 0,
            scroll_to_row: None,
            results: Arc::new(RwLock::new(SearchResults::default())),
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }
}

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
}

impl Default for FastCsvApp {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(SharedState::default())),
            scroll_y: 0.0,
            scroll_x: 0.0,
            column_widths: Vec::new(),
            row_cache: std::collections::HashMap::new(),
            last_visible_range: (0, 0),
            search: SearchState::default(),
        }
    }
}

impl FastCsvApp {
    /// Open a file dialog and load the selected CSV file
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

    /// Load a CSV file in the background
    fn load_file(&mut self, path: PathBuf, ctx: egui::Context) {
        // Reset state
        self.scroll_y = 0.0;
        self.scroll_x = 0.0;
        self.column_widths.clear();
        self.row_cache.clear();
        self.last_visible_range = (0, 0);
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
        }

        let state = Arc::clone(&self.state);
        let path_clone = path.clone();

        // Spawn background thread for indexing
        thread::spawn(move || {
            let result = load_and_index_csv(&path_clone, &state);

            let mut state_guard = state.write();
            match result {
                Ok(mapped_csv) => {
                    state_guard.csv = Some(mapped_csv);
                    state_guard.load_state = LoadState::Ready;
                }
                Err(e) => {
                    state_guard.load_state = LoadState::Error;
                    state_guard.error_message = Some(e);
                }
            }
            drop(state_guard);

            // Request UI repaint
            ctx.request_repaint();
        });
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
            state.csv.as_ref().map(|c| c.total_rows).unwrap_or(0)
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

            for row_idx in 0..csv.total_rows {
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
            results.rows_searched = csv.total_rows;
            results.status = SearchStatus::Complete;
            results.nav_limit_reached = nav_limit_reached;
            drop(results);
            ctx.request_repaint();
        });
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

            // Execute search on Enter
            if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                self.execute_search(ctx);
            }

            // Search button (disabled during search)
            let is_searching = status == SearchStatus::Searching;
            if ui
                .add_enabled(!is_searching, egui::Button::new("Search"))
                .clicked()
            {
                self.execute_search(ctx);
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
                if ui.button("✕").clicked() {
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
        let (headers, total_rows, num_columns) = {
            let state = self.state.read();
            match &state.csv {
                Some(csv) => (csv.headers.clone(), csv.total_rows, csv.headers.len()),
                None => return,
            }
        };

        // Ensure we have column widths
        if self.column_widths.len() != num_columns {
            self.column_widths = vec![DEFAULT_COLUMN_WIDTH; num_columns];
        }

        let _text_color = ui.style().visuals.text_color();

        // Handle scroll to row request
        let scroll_to_row = self.search.scroll_to_row.take();

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

                // Add columns with initial width - use clip(true) to prevent overflow
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

                table
                    .header(ROW_HEIGHT, |mut header| {
                        for col_name in &headers {
                            header.col(|ui| {
                                ui.strong(col_name);
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(ROW_HEIGHT, total_rows, |mut row| {
                            let row_idx = row.index();

                            // Get or parse row data
                            let fields = if let Some(cached) = self.row_cache.get(&row_idx) {
                                cached.clone()
                            } else {
                                // Parse from mmap
                                let state = self.state.read();
                                let fields = if let Some(csv) = &state.csv {
                                    csv.parse_row(row_idx).unwrap_or_default()
                                } else {
                                    vec![]
                                };
                                drop(state);

                                // Cache for next render
                                self.row_cache.insert(row_idx, fields.clone());
                                fields
                            };

                            // Check if this is the current navigation row (for special highlighting)
                            let is_current_nav_row = current_nav_row == Some(row_idx);

                            // Render each cell with ON-THE-FLY search highlighting
                            // This is O(visible_rows) per frame - no memory overhead!
                            for field in fields.iter() {
                                row.col(|ui| {
                                    // Check if this cell matches the search query
                                    let is_match = !search_query.is_empty()
                                        && field.to_lowercase().contains(&search_query);

                                    if is_match && is_current_nav_row {
                                        // Current navigation row match - bright orange
                                        let rect = ui.available_rect_before_wrap();
                                        ui.painter().rect_filled(rect, 2.0, CURRENT_MATCH_COLOR);
                                        ui.label(egui::RichText::new(field).color(Color32::BLACK));
                                    } else if is_match {
                                        // Other matches - yellow background
                                        let rect = ui.available_rect_before_wrap();
                                        ui.painter().rect_filled(rect, 2.0, HIGHLIGHT_COLOR);
                                        ui.label(egui::RichText::new(field).color(Color32::BLACK));
                                    } else {
                                        ui.label(field);
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
                    ui.label(format!("Rows: {}", format_number(csv.total_rows)));
                    ui.separator();
                    ui.label(format!("Columns: {}", csv.headers.len()));
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
}

impl eframe::App for FastCsvApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Handle keyboard shortcuts
        ctx.input(|i| {
            // Cmd/Ctrl+F to toggle search
            if i.modifiers.command && i.key_pressed(Key::F) {
                self.search.visible = !self.search.visible;
                if !self.search.visible {
                    // Cancel and clear search when closing
                    self.search.cancel_flag.store(true, Ordering::SeqCst);
                    self.search.query.clear();
                    self.search.active_query.clear();
                    let mut results = self.search.results.write();
                    results.navigation_rows.clear();
                    results.total_match_count = 0;
                    results.status = SearchStatus::Idle;
                }
            }
            // Escape to close search
            if i.key_pressed(Key::Escape) && self.search.visible {
                self.search.cancel_flag.store(true, Ordering::SeqCst);
                self.search.visible = false;
                self.search.query.clear();
                self.search.active_query.clear();
                let mut results = self.search.results.write();
                results.navigation_rows.clear();
                results.total_match_count = 0;
                results.status = SearchStatus::Idle;
            }
            // F3 or Cmd+G for next match
            if i.key_pressed(Key::F3) || (i.modifiers.command && i.key_pressed(Key::G)) {
                if i.modifiers.shift {
                    self.prev_match();
                } else {
                    self.next_match();
                }
            }
        });

        // Top panel with menu/toolbar
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open...").clicked() {
                        ui.close_menu();
                        self.open_file(ctx);
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if ui.button("Find... (⌘F)").clicked() {
                        ui.close_menu();
                        self.search.visible = true;
                    }
                    let has_nav_rows = !self.search.results.read().navigation_rows.is_empty();
                    if ui
                        .add_enabled(has_nav_rows, egui::Button::new("Find Next (F3)"))
                        .clicked()
                    {
                        ui.close_menu();
                        self.next_match();
                    }
                    if ui
                        .add_enabled(has_nav_rows, egui::Button::new("Find Previous (⇧F3)"))
                        .clicked()
                    {
                        ui.close_menu();
                        self.prev_match();
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

        // Handle dropped files
        ctx.input(|i| {
            for file in &i.raw.dropped_files {
                if let Some(path) = &file.path {
                    self.load_file(path.clone(), ctx.clone());
                    break;
                }
            }
        });
    }
}

/// Load and index a CSV file in the background
fn load_and_index_csv(
    path: &PathBuf,
    state: &Arc<RwLock<SharedState>>,
) -> Result<MappedCsv, String> {
    // Open and memory-map the file
    let file = File::open(path).map_err(|e| format!("Failed to open file: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("Failed to read metadata: {e}"))?;
    let file_size = metadata.len();

    if file_size == 0 {
        return Err("File is empty".to_string());
    }

    // Safety: We're only reading the file, and it won't be modified while we have it mapped
    let mmap =
        unsafe { Mmap::map(&file) }.map_err(|e| format!("Failed to memory-map file: {e}"))?;

    // Index row offsets by scanning for newlines
    let mut row_offsets = Vec::with_capacity((file_size / 50) as usize); // Estimate ~50 bytes per row
    row_offsets.push(0); // First row starts at byte 0

    let cancel_flag = &state.read().cancel_indexing;
    let rows_indexed = &state.read().rows_indexed;

    let mut offset = 0;
    let bytes = &mmap[..];

    while offset < bytes.len() {
        // Check for cancellation
        if cancel_flag.load(Ordering::Relaxed) {
            return Err("Indexing cancelled".to_string());
        }

        // Find next newline
        if let Some(pos) = memchr::memchr(b'\n', &bytes[offset..]) {
            offset += pos + 1;
            if offset < bytes.len() {
                row_offsets.push(offset);
            }

            // Update progress every 10000 rows
            if row_offsets.len() % 10000 == 0 {
                rows_indexed.store(row_offsets.len(), Ordering::Relaxed);
            }
        } else {
            break;
        }
    }

    // Final row count update
    rows_indexed.store(row_offsets.len(), Ordering::Relaxed);

    if row_offsets.len() < 2 {
        return Err("File has no data rows".to_string());
    }

    // Parse headers from first row
    let header_end = row_offsets.get(1).copied().unwrap_or(bytes.len());
    let header_bytes = &bytes[0..header_end];

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(header_bytes);

    let headers: Vec<String> = reader
        .records()
        .next()
        .and_then(|r| r.ok())
        .map(|record| record.iter().map(|s| s.to_string()).collect())
        .unwrap_or_else(|| vec!["Column".to_string()]);

    let total_rows = row_offsets.len() - 1; // Subtract 1 for header

    Ok(MappedCsv {
        mmap,
        row_offsets,
        headers,
        total_rows,
        path: path.clone(),
        file_size,
    })
}

/// Format a number with thousand separators
fn format_number(n: usize) -> String {
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
fn format_file_size(bytes: u64) -> String {
    use humansize::{format_size, BINARY};
    format_size(bytes, BINARY)
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([600.0, 400.0])
            .with_title("QuickCSV")
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "QuickCSV",
        options,
        Box::new(|cc| {
            // Follow system theme
            cc.egui_ctx.set_visuals(egui::Visuals::dark());

            Ok(Box::new(FastCsvApp::default()))
        }),
    )
}
