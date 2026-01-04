//! Memory-mapped CSV file structure

use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use memmap2::Mmap;

#[cfg(not(target_arch = "wasm32"))]
pub type DataStore = Mmap;

#[cfg(target_arch = "wasm32")]
pub type DataStore = Vec<u8>;

/// Memory-mapped CSV file with row offset index
pub struct MappedCsv {
    /// Memory-mapped file data (Native) or Byte vector (Web)
    pub(crate) data: DataStore,
    /// Byte offset of each row (including header) - shared for progressive loading
    pub row_offsets: Arc<RwLock<Vec<usize>>>,
    /// Column headers
    pub headers: Vec<String>,
    /// File path
    pub path: PathBuf,
    /// File size in bytes
    pub file_size: u64,
    /// Detected delimiter (comma, tab, semicolon, pipe)
    pub delimiter: u8,
}

impl MappedCsv {
    /// Get current number of indexed rows (excluding header)
    pub fn indexed_row_count(&self) -> usize {
        let offsets = self.row_offsets.read();
        if offsets.len() > 1 {
            offsets.len() - 1 // Subtract 1 for header
        } else {
            0
        }
    }

    /// Get a row's data as a slice of the memory-mapped region
    pub fn get_row_bytes(&self, row_index: usize) -> Option<&[u8]> {
        let offsets = self.row_offsets.read();

        // Row 0 is the header, so data rows start at index 1
        let data_row_offset_index = row_index + 1;
        if data_row_offset_index >= offsets.len() {
            return None;
        }

        let start = offsets[data_row_offset_index];
        let end = if data_row_offset_index + 1 < offsets.len() {
            offsets[data_row_offset_index + 1]
        } else {
            self.data.len()
        };

        Some(&self.data[start..end])
    }

    /// Parse a single row into fields
    pub fn parse_row(&self, row_index: usize) -> Option<Vec<String>> {
        let bytes = self.get_row_bytes(row_index)?;
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .delimiter(self.delimiter)
            .from_reader(bytes);

        reader.records().next().and_then(|result| {
            result
                .ok()
                .map(|record| record.iter().map(|s| s.to_string()).collect())
        })
    }

    /// Get human-readable delimiter name for display
    pub fn delimiter_name(&self) -> &'static str {
        match self.delimiter {
            b',' => "Comma",
            b'\t' => "Tab",
            b';' => "Semicolon",
            b'|' => "Pipe",
            _ => "Unknown",
        }
    }
}
