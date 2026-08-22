//! CSV parsing functions
//!
//! Contains functions for detecting delimiters, finding row boundaries,
//! and progressive loading of CSV files.

#[cfg(not(target_arch = "wasm32"))]
use super::MappedCsv;
#[cfg(not(target_arch = "wasm32"))]
use crate::state::{LoadState, SharedState};
#[cfg(target_arch = "wasm32")]
use eframe::egui;
#[cfg(not(target_arch = "wasm32"))]
use eframe::egui;
#[cfg(not(target_arch = "wasm32"))]
use memmap2::Mmap;
#[cfg(not(target_arch = "wasm32"))]
use parking_lot::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use std::fs::File;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{BufReader, Read};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::thread;

#[cfg(not(target_arch = "wasm32"))]
static SNAPSHOT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(not(target_arch = "wasm32"))]
fn snapshot_directory_name() -> String {
    let id = SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(".quickcsv-{}-{id}.snapshot.d", std::process::id())
}

#[cfg(not(target_arch = "wasm32"))]
struct SnapshotCleanup {
    directory: PathBuf,
    path: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
fn try_create_snapshot_workspace(parent: &Path) -> Option<SnapshotCleanup> {
    for _ in 0..16 {
        let directory = parent.join(snapshot_directory_name());
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&directory) {
            Ok(()) => {
                return Some(SnapshotCleanup {
                    path: directory.join("data"),
                    directory,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn create_temp_snapshot_workspace() -> Result<SnapshotCleanup, String> {
    try_create_snapshot_workspace(&std::env::temp_dir())
        .ok_or_else(|| "Failed to create an exclusive snapshot directory".to_string())
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for SnapshotCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

#[cfg(target_os = "macos")]
fn try_clone_snapshot(source: &Path, destination: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let Ok(source) = CString::new(source.as_os_str().as_bytes()) else {
        return false;
    };
    let Ok(destination) = CString::new(destination.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::clonefile(source.as_ptr(), destination.as_ptr(), 0) == 0 }
}

#[cfg(target_os = "linux")]
fn try_clone_snapshot(source: &Path, destination: &Path) -> bool {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    const FICLONE: libc::c_ulong = 0x4004_9409;
    let Ok(source) = File::open(source) else {
        return false;
    };
    let Ok(destination_file) = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)
    else {
        return false;
    };
    let cloned =
        unsafe { libc::ioctl(destination_file.as_raw_fd(), FICLONE, source.as_raw_fd()) == 0 };
    if !cloned {
        drop(destination_file);
        let _ = std::fs::remove_file(destination);
    }
    cloned
}

#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "macos", target_os = "linux"))
))]
fn try_clone_snapshot(_source: &Path, _destination: &Path) -> bool {
    false
}

/// Create an immutable disk-backed snapshot so external truncation cannot
/// invalidate pages already mapped by QuickCSV. APFS uses a constant-time COW
/// clone; other native platforms fall back to a bounded-memory file copy.
#[cfg(not(target_arch = "wasm32"))]
fn files_equal(left: &Path, right: &Path) -> Result<bool, String> {
    let mut left = BufReader::new(
        File::open(left).map_err(|error| format!("Failed to verify source file: {error}"))?,
    );
    let mut right = BufReader::new(
        File::open(right).map_err(|error| format!("Failed to verify snapshot file: {error}"))?,
    );
    let mut left_buffer = vec![0_u8; 1024 * 1024];
    let mut right_buffer = vec![0_u8; 1024 * 1024];
    loop {
        let left_read = left
            .read(&mut left_buffer)
            .map_err(|error| format!("Failed while verifying source file: {error}"))?;
        let right_read = right
            .read(&mut right_buffer)
            .map_err(|error| format!("Failed while verifying snapshot file: {error}"))?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn source_changed(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    before.len() != after.len() || before.modified().ok() != after.modified().ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn open_snapshot(source: &Path) -> Result<(File, SnapshotCleanup), String> {
    const MAX_ATTEMPTS: usize = 3;

    for _ in 0..MAX_ATTEMPTS {
        if let Some(parent) = source.parent() {
            if let Some(cleanup) = try_create_snapshot_workspace(parent) {
                if try_clone_snapshot(source, &cleanup.path) {
                    let file = File::open(&cleanup.path)
                        .map_err(|error| format!("Failed to open file snapshot: {error}"))?;
                    return Ok((file, cleanup));
                }
            }
        }

        let cleanup = create_temp_snapshot_workspace()?;
        let before = std::fs::metadata(source)
            .map_err(|e| format!("Failed to read source metadata: {e}"))?;
        let cloned = try_clone_snapshot(source, &cleanup.path);
        if !cloned {
            std::fs::copy(source, &cleanup.path)
                .map_err(|e| format!("Failed to snapshot file: {e}"))?;
            let after = std::fs::metadata(source)
                .map_err(|e| format!("Failed to verify source metadata: {e}"))?;
            if source_changed(&before, &after) || !files_equal(source, &cleanup.path)? {
                continue;
            }
            let verified = std::fs::metadata(source)
                .map_err(|e| format!("Failed to verify source metadata: {e}"))?;
            if source_changed(&after, &verified) {
                continue;
            }
        }

        match File::open(&cleanup.path) {
            Ok(file) => return Ok((file, cleanup)),
            Err(error) => return Err(format!("Failed to open file snapshot: {error}")),
        }
    }

    Err("File kept changing while QuickCSV was creating a stable snapshot".to_string())
}

/// Find the next row boundary in CSV data, respecting quoted fields.
///
/// RFC 4180 compliant: newlines inside quoted fields are NOT row boundaries.
/// Handles escaped quotes ("") correctly by toggle-counting.
///
/// Returns the byte offset of the start of the next row, or None if no more rows.
pub fn find_row_boundary(bytes: &[u8], start: usize) -> Option<usize> {
    let mut in_quotes = false;
    let mut i = start;

    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                // Toggle quote state. For escaped quotes (""), this toggles twice,
                // which correctly maintains the quote state.
                in_quotes = !in_quotes;
            }
            b'\n' if !in_quotes => {
                // Found a row boundary (newline outside of quotes)
                return Some(i + 1);
            }
            b'\r' if !in_quotes => {
                // Handle CRLF: if next char is \n, skip both
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    return Some(i + 2);
                }
                // Standalone \r (old Mac format) - treat as row boundary
                return Some(i + 1);
            }
            _ => {}
        }
        i += 1;
    }

    // No more newlines found - if we're past the start and at EOF, that's the last row
    if i > start {
        Some(i)
    } else {
        None
    }
}

/// Detect the delimiter used in a CSV file by analyzing the first few lines
///
/// Algorithm:
/// 1. Read the first 5 lines
/// 2. Count occurrences of common delimiters: comma, tab, semicolon, pipe
/// 3. Pick the delimiter that appears most consistently across lines
/// 4. Fall back to comma if no clear winner
pub fn detect_delimiter(bytes: &[u8]) -> u8 {
    const CANDIDATES: [u8; 4] = *b",\t;|";
    const MAX_LINES: usize = 5;
    const MAX_BYTES: usize = 10000; // Don't scan too much

    let scan_bytes = &bytes[..bytes.len().min(MAX_BYTES)];

    // Split into lines (simple split, not quote-aware for speed)
    let mut lines: Vec<&[u8]> = Vec::new();
    let mut start = 0;
    for (i, &b) in scan_bytes.iter().enumerate() {
        if b == b'\n' {
            if i > start {
                lines.push(&scan_bytes[start..i]);
            }
            start = i + 1;
            if lines.len() >= MAX_LINES {
                break;
            }
        }
    }
    // Add last line if no trailing newline
    if start < scan_bytes.len() && lines.len() < MAX_LINES {
        lines.push(&scan_bytes[start..]);
    }

    if lines.is_empty() {
        return b','; // Default to comma
    }

    // Count each delimiter in each line
    let mut best_delimiter = b',';
    let mut best_score: i32 = -1;

    for &delim in &CANDIDATES {
        let counts: Vec<usize> = lines
            .iter()
            .map(|line| {
                // Count delimiter occurrences (simple count, ignoring quotes for speed)
                line.iter().filter(|&&b| b == delim).count()
            })
            .collect();

        // Skip if no occurrences
        if counts.iter().all(|&c| c == 0) {
            continue;
        }

        // Calculate score based on consistency
        // Higher score = more consistent counts across lines
        let first_count = counts[0];
        let all_same = counts.iter().all(|&c| c == first_count && c > 0);
        let total: usize = counts.iter().sum();

        let score = if all_same && first_count > 0 {
            // Perfect consistency: high score
            (total * 10) as i32
        } else if first_count > 0 {
            // Some occurrences but inconsistent
            total as i32
        } else {
            0
        };

        if score > best_score {
            best_score = score;
            best_delimiter = delim;
        }
    }

    best_delimiter
}

/// Initialize CSV file for progressive loading - parses headers and starts indexing
#[cfg(not(target_arch = "wasm32"))]
pub fn init_csv_progressive(
    path: &Path,
    state: &Arc<RwLock<SharedState>>,
    cancel_flag: &Arc<AtomicBool>,
    ctx: &egui::Context,
    progressive: bool,
) -> Result<(), String> {
    let (file, snapshot) = open_snapshot(path)?;
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            drop(snapshot);
            return Err(format!("Failed to read metadata: {error}"));
        }
    };
    let file_size = metadata.len();

    if file_size == 0 {
        drop(snapshot);
        return Err("File is empty".to_string());
    }

    // The mapping is read-only and shared by the UI and indexer for this load generation.
    // The file watcher cancels and unpublishes the generation on external changes.
    let mmap_result = unsafe { Mmap::map(&file) };
    drop(snapshot);
    let mmap = Arc::new(mmap_result.map_err(|e| format!("Failed to memory-map file: {e}"))?);

    if cancel_flag.load(Ordering::Acquire) {
        return Ok(());
    }

    let bytes = &mmap[..];

    // Detect delimiter from file content
    let delimiter = detect_delimiter(bytes);

    // Find the first row boundary (quote-aware) to get headers
    let first_row_end = find_row_boundary(bytes, 0).unwrap_or(bytes.len());
    // Strip trailing newline/CRLF for header parsing
    let header_end = if first_row_end > 0 && first_row_end <= bytes.len() {
        let mut end = first_row_end;
        if end > 0 && bytes.get(end - 1) == Some(&b'\n') {
            end -= 1;
        }
        if end > 0 && bytes.get(end - 1) == Some(&b'\r') {
            end -= 1;
        }
        end
    } else {
        first_row_end
    };

    // Parse headers from first row using detected delimiter
    let header_bytes = &bytes[0..header_end];
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(delimiter)
        .from_reader(header_bytes);

    let headers: Vec<String> = reader
        .records()
        .next()
        .and_then(|r| r.ok())
        .map(|record| record.iter().map(|s| s.to_string()).collect())
        .unwrap_or_else(|| vec!["Column".to_string()]);

    // Create shared row_offsets with just the header and first data row
    let row_offsets = Arc::new(RwLock::new(vec![0, first_row_end]));

    // Create MappedCsv immediately so UI can show data
    let mapped_csv = MappedCsv {
        mmap: Arc::clone(&mmap),
        row_offsets: Arc::clone(&row_offsets),
        headers,
        path: path.to_path_buf(),
        file_size,
        delimiter,
    };

    // Update state to Ready immediately so UI shows data
    {
        let mut state_guard = state.write();
        state_guard.csv = Some(mapped_csv);
        state_guard.load_state = if progressive {
            LoadState::Ready
        } else {
            LoadState::Indexing
        };
        state_guard.rows_indexed.store(1, Ordering::Relaxed);
        state_guard
            .indexing_complete
            .store(false, Ordering::Relaxed);
    }

    // Request UI repaint to show the data immediately
    ctx.request_repaint();

    // Continue indexing in background - we need to re-map the file in the background thread
    // because the mmap in MappedCsv is already used by the UI
    let state_clone = Arc::clone(state);
    let ctx_clone = ctx.clone();
    let cancel_flag = Arc::clone(cancel_flag);
    let start_offset = first_row_end;

    thread::spawn(move || {
        let bytes = &mmap[..];

        let row_offsets_ref = {
            let state_guard = state_clone.read();
            let csv = match &state_guard.csv {
                Some(csv) => csv,
                None => return,
            };
            Arc::clone(&csv.row_offsets)
        };

        let mut offset = start_offset;
        let mut batch_offsets = Vec::with_capacity(10000);

        while offset < bytes.len() {
            // Check for cancellation
            if cancel_flag.load(Ordering::Acquire) {
                return;
            }

            // Find next row boundary (quote-aware for RFC 4180 compliance)
            if let Some(next_offset) = find_row_boundary(bytes, offset) {
                if next_offset > offset && next_offset < bytes.len() {
                    batch_offsets.push(next_offset);
                }
                offset = next_offset;

                // If we're at EOF, stop
                if offset >= bytes.len() {
                    break;
                }

                // Batch update every 10000 rows for better performance
                if batch_offsets.len() >= 10000 {
                    {
                        let mut offsets = row_offsets_ref.write();
                        offsets.extend(batch_offsets.drain(..));
                        let count = offsets.len();
                        drop(offsets);

                        let state_guard = state_clone.read();
                        state_guard.rows_indexed.store(count, Ordering::Relaxed);
                    }
                    ctx_clone.request_repaint();
                }
            } else {
                break;
            }
        }

        // Final batch
        if !batch_offsets.is_empty() {
            let mut offsets = row_offsets_ref.write();
            offsets.extend(batch_offsets);
            let count = offsets.len();
            drop(offsets);

            let state_guard = state_clone.read();
            state_guard.rows_indexed.store(count, Ordering::Relaxed);
        }

        // Mark indexing as complete
        {
            if cancel_flag.load(Ordering::Acquire) {
                return;
            }
            let mut state_guard = state_clone.write();
            state_guard.indexing_complete.store(true, Ordering::SeqCst);
            state_guard.load_state = LoadState::Ready;
        }
        ctx_clone.request_repaint();
    });

    Ok(())
}

/// Initialize CSV from in-memory bytes (WASM)
/// This function runs synchronously but is only called after file is fully loaded
#[cfg(target_arch = "wasm32")]
pub fn init_csv_web(
    name: String,
    bytes: Vec<u8>,
    state: &std::sync::Arc<parking_lot::RwLock<crate::state::SharedState>>,
    ctx: &egui::Context,
) -> Result<(), String> {
    use crate::state::LoadState;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    let file_size = bytes.len() as u64;

    if file_size == 0 {
        return Err("File is empty".to_string());
    }

    // Detect delimiter
    let delimiter = detect_delimiter(&bytes);

    // Find the first row boundary to get headers
    let first_row_end = find_row_boundary(&bytes, 0).unwrap_or(bytes.len());

    // Header parsing
    let header_end = if first_row_end > 0 && first_row_end <= bytes.len() {
        let mut end = first_row_end;
        if end > 0 && bytes.get(end - 1) == Some(&b'\n') {
            end -= 1;
        }
        if end > 0 && bytes.get(end - 1) == Some(&b'\r') {
            end -= 1;
        }
        end
    } else {
        first_row_end
    };

    let header_bytes = &bytes[0..header_end];
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(delimiter)
        .from_reader(header_bytes);

    let headers: Vec<String> = reader
        .records()
        .next()
        .and_then(|r| r.ok())
        .map(|record| record.iter().map(|s| s.to_string()).collect())
        .unwrap_or_else(|| vec!["Column".to_string()]);

    let row_offsets = Arc::new(parking_lot::RwLock::new(vec![0, first_row_end]));

    // Index the file - for large files this may briefly freeze UI
    // but it's much safer than async allocation patterns
    let mut offset = first_row_end;
    let mut batch_offsets = Vec::with_capacity(10000);

    while offset < bytes.len() {
        if let Some(next_offset) = find_row_boundary(&bytes, offset) {
            if next_offset > offset && next_offset < bytes.len() {
                batch_offsets.push(next_offset);
            }
            offset = next_offset;
            if offset >= bytes.len() {
                break;
            }
        } else {
            break;
        }
    }

    {
        let mut offsets = row_offsets.write();
        offsets.extend(batch_offsets);
    }

    let total_rows = row_offsets.read().len();

    let mapped_csv = super::MappedCsv {
        data: bytes,
        row_offsets,
        headers,
        path: PathBuf::from(name),
        file_size,
        delimiter,
    };

    // Update state to Ready
    {
        let mut state_guard = state.write();
        state_guard.csv = Some(mapped_csv);
        state_guard.load_state = LoadState::Ready;
        state_guard
            .rows_indexed
            .store(total_rows, Ordering::Relaxed);
        state_guard.indexing_complete.store(true, Ordering::Relaxed);
    }

    ctx.request_repaint();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // find_row_boundary tests (RFC 4180 compliance)
    // -------------------------------------------------------------------------

    #[test]
    fn test_find_row_boundary_simple() {
        let data = b"row1\nrow2\nrow3";
        assert_eq!(find_row_boundary(data, 0), Some(5)); // After "row1\n"
        assert_eq!(find_row_boundary(data, 5), Some(10)); // After "row2\n"
    }

    #[test]
    fn test_find_row_boundary_crlf() {
        let data = b"row1\r\nrow2\r\n";
        assert_eq!(find_row_boundary(data, 0), Some(6)); // After "row1\r\n"
        assert_eq!(find_row_boundary(data, 6), Some(12)); // After "row2\r\n"
    }

    #[test]
    fn test_find_row_boundary_quoted_newline() {
        // Newline inside quotes should NOT be a row boundary
        let data = b"\"hello\nworld\",value\nnext";
        assert_eq!(find_row_boundary(data, 0), Some(20)); // After the full first row
    }

    #[test]
    fn test_find_row_boundary_escaped_quotes() {
        // Escaped quotes ("") inside quoted field
        let data = b"\"say \"\"hello\"\"\",value\nnext";
        assert_eq!(find_row_boundary(data, 0), Some(22));
    }

    #[test]
    fn test_find_row_boundary_no_newline() {
        let data = b"single row no newline";
        assert_eq!(find_row_boundary(data, 0), Some(21)); // EOF
    }

    #[test]
    fn test_find_row_boundary_empty() {
        let data = b"";
        assert_eq!(find_row_boundary(data, 0), None);
    }

    #[test]
    fn test_find_row_boundary_single_char() {
        let data = b"a";
        assert_eq!(find_row_boundary(data, 0), Some(1)); // EOF
    }

    #[test]
    fn test_find_row_boundary_only_newline() {
        let data = b"\n";
        assert_eq!(find_row_boundary(data, 0), Some(1));
    }

    #[test]
    fn test_find_row_boundary_only_crlf() {
        let data = b"\r\n";
        assert_eq!(find_row_boundary(data, 0), Some(2));
    }

    #[test]
    fn test_find_row_boundary_complex_quoted() {
        // Multiple quoted fields with embedded newlines
        let data = b"\"a\nb\",\"c\nd\"\nrow2";
        let boundary = find_row_boundary(data, 0);
        assert_eq!(boundary, Some(12)); // After first complete row
    }

    // -------------------------------------------------------------------------
    // detect_delimiter tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_detect_delimiter_comma() {
        let data = b"name,age,city\nJohn,30,NYC\nJane,25,LA\n";
        assert_eq!(detect_delimiter(data), b',');
    }

    #[test]
    fn test_detect_delimiter_tab() {
        let data = b"name\tage\tcity\nJohn\t30\tNYC\nJane\t25\tLA\n";
        assert_eq!(detect_delimiter(data), b'\t');
    }

    #[test]
    fn test_detect_delimiter_semicolon() {
        let data = b"name;age;city\nJohn;30;NYC\nJane;25;LA\n";
        assert_eq!(detect_delimiter(data), b';');
    }

    #[test]
    fn test_detect_delimiter_pipe() {
        let data = b"name|age|city\nJohn|30|NYC\nJane|25|LA\n";
        assert_eq!(detect_delimiter(data), b'|');
    }

    #[test]
    fn test_detect_delimiter_default_comma() {
        // No clear delimiter, should default to comma
        let data = b"just some text\nmore text here\n";
        assert_eq!(detect_delimiter(data), b',');
    }

    #[test]
    fn test_detect_delimiter_empty() {
        let data = b"";
        assert_eq!(detect_delimiter(data), b','); // Default to comma
    }

    #[test]
    fn test_detect_delimiter_single_line() {
        let data = b"a,b,c";
        assert_eq!(detect_delimiter(data), b',');
    }

    #[test]
    fn test_detect_delimiter_mixed() {
        // File with both comma and semicolon but comma is more consistent
        let data = b"a,b,c\n1,2,3\n4,5,6\n";
        assert_eq!(detect_delimiter(data), b',');
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn snapshot_mapping_survives_source_truncation() {
        let source = std::env::temp_dir().join(format!(
            "quickcsv-snapshot-test-{}",
            SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let content = vec![b'x'; 16 * 1024];
        std::fs::write(&source, &content).expect("should create source file");

        let (snapshot_file, snapshot) = open_snapshot(&source).expect("snapshot should open");
        let mmap = unsafe { Mmap::map(&snapshot_file) }.expect("snapshot should map");
        drop(snapshot);
        File::create(&source).expect("source should truncate");

        assert_eq!(mmap.len(), content.len());
        assert_eq!(mmap[content.len() - 1], b'x');

        let _ = std::fs::remove_file(source);
    }
}
