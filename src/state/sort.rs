//! Sort state for column sorting

use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;

/// Sort direction for column sorting
#[derive(Clone, Copy, PartialEq, Default)]
pub enum SortDirection {
    #[default]
    None,
    Ascending,
    Descending,
}

/// Column sort state
pub struct SortState {
    /// Currently sorted column index (None if not sorted)
    pub column: Option<usize>,
    /// Sort direction
    pub direction: SortDirection,
    /// Sorted row indices (maps display index -> actual row index)
    pub sorted_indices: Arc<RwLock<Vec<usize>>>,
    /// Whether sorting is in progress
    pub is_sorting: Arc<AtomicBool>,
    /// Sorting progress (0-100)
    pub progress: Arc<AtomicUsize>,
    /// Cancel flag for sorting
    pub cancel_flag: Arc<AtomicBool>,
}

impl Default for SortState {
    fn default() -> Self {
        Self {
            column: None,
            direction: SortDirection::None,
            sorted_indices: Arc::new(RwLock::new(Vec::new())),
            is_sorting: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(AtomicUsize::new(0)),
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }
}
