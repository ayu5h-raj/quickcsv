//! FastCSV - High-Performance CSV Viewer for macOS
//!
//! A memory-mapped, virtualized CSV viewer that can handle files from 100MB to 2GB+
//! with zero lag. Uses memmap2 for zero-copy file loading and egui for the UI.

use eframe::egui;
use memmap2::Mmap;
use parking_lot::RwLock;
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;


/// Height of each row in pixels
const ROW_HEIGHT: f32 = 24.0;

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
            result.ok().map(|record| {
                record.iter().map(|s| s.to_string()).collect()
            })
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

                // Add columns with initial width - use clip(true) to prevent overflow
                for col_width in &self.column_widths {
                    table = table.column(Column::initial(*col_width).resizable(true).clip(true));
                }

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

                            // Render each cell
                            for field in &fields {
                                row.col(|ui| {
                                    ui.label(field);
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

        ui.horizontal(|ui| {
            match state.load_state {
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
            }
        });
    }
}

impl eframe::App for FastCsvApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
            });
        });

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
                            ui.heading("FastCSV");
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
                            ui.label(format!("Indexing file... {} rows found", format_number(rows)));
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
                                format!("Error: {}", state.error_message.as_deref().unwrap_or("Unknown")),
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
fn load_and_index_csv(path: &PathBuf, state: &Arc<RwLock<SharedState>>) -> Result<MappedCsv, String> {
    // Open and memory-map the file
    let file = File::open(path).map_err(|e| format!("Failed to open file: {}", e))?;
    let metadata = file.metadata().map_err(|e| format!("Failed to read metadata: {}", e))?;
    let file_size = metadata.len();

    if file_size == 0 {
        return Err("File is empty".to_string());
    }

    // Safety: We're only reading the file, and it won't be modified while we have it mapped
    let mmap = unsafe { Mmap::map(&file) }.map_err(|e| format!("Failed to memory-map file: {}", e))?;

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
            .with_title("FastCSV")
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "FastCSV",
        options,
        Box::new(|cc| {
            // Follow system theme
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            
            Ok(Box::new(FastCsvApp::default()))
        }),
    )
}

