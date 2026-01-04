//! CSV handling module
//!
//! This module contains the memory-mapped CSV structure and parsing functions.

mod mapped;
mod parser;

pub use mapped::MappedCsv;
pub use parser::{detect_delimiter, find_row_boundary, init_csv_progressive};
