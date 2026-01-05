//! Application state module
//!
//! This module contains all state structs used by the application.

mod columns;
mod dialogs;
mod search;
mod shared;
mod sort;

pub use columns::{ColumnAction, ColumnState};
pub use dialogs::{GoToRowState, JsonViewerState, RowDetailState};
pub use search::{SearchState, SearchStatus, MAX_NAV_ROWS};
pub use shared::{LoadState, SharedState};
pub use sort::{SortDirection, SortState};
