//! CSV handling module
//!
//! This module contains the memory-mapped CSV structure and parsing functions.

mod mapped;
pub mod parser;

pub use mapped::MappedCsv;
#[cfg(not(target_arch = "wasm32"))]
pub use parser::init_csv_progressive;
