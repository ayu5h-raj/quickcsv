//! Shared state between UI thread and background workers

use crate::csv::MappedCsv;
use std::sync::atomic::{AtomicBool, AtomicUsize};

/// State of CSV file loading and indexing
#[derive(Clone, Copy, PartialEq)]
pub enum LoadState {
    Empty,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    ReloadPending,
    Indexing,
    Ready,
    Error,
}

/// Shared state between UI thread and background indexer
pub struct SharedState {
    /// The loaded CSV file (None if no file loaded)
    pub csv: Option<MappedCsv>,
    /// Current loading state
    pub load_state: LoadState,
    /// Error message if loading failed
    pub error_message: Option<String>,
    /// Number of rows indexed so far (for progress display)
    pub rows_indexed: AtomicUsize,
    /// Flag to indicate indexing is complete
    pub indexing_complete: AtomicBool,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            csv: None,
            load_state: LoadState::Empty,
            error_message: None,
            rows_indexed: AtomicUsize::new(0),
            indexing_complete: AtomicBool::new(false),
        }
    }
}
