//! Filter state management for column filtering

use std::collections::HashMap;

/// Filter operators for column filtering
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterOperator {
    Contains,
    Equals,
    StartsWith,
    EndsWith,
    GreaterThan,
    LessThan,
    NotEmpty,
    Empty,
}

impl FilterOperator {
    /// Get display name for the operator
    pub fn display_name(&self) -> &'static str {
        match self {
            FilterOperator::Contains => "Contains",
            FilterOperator::Equals => "Equals",
            FilterOperator::StartsWith => "Starts with",
            FilterOperator::EndsWith => "Ends with",
            FilterOperator::GreaterThan => "Greater than",
            FilterOperator::LessThan => "Less than",
            FilterOperator::NotEmpty => "Not empty",
            FilterOperator::Empty => "Is empty",
        }
    }

    /// Get all operators for UI dropdown
    pub fn all() -> &'static [FilterOperator] {
        &[
            FilterOperator::Contains,
            FilterOperator::Equals,
            FilterOperator::StartsWith,
            FilterOperator::EndsWith,
            FilterOperator::GreaterThan,
            FilterOperator::LessThan,
            FilterOperator::NotEmpty,
            FilterOperator::Empty,
        ]
    }
}

impl Default for FilterOperator {
    fn default() -> Self {
        FilterOperator::Contains
    }
}

/// A filter condition for a column
#[derive(Clone, Debug, PartialEq)]
pub struct FilterCondition {
    pub operator: FilterOperator,
    pub value: String,
}

impl FilterCondition {
    /// Check if a cell value matches this filter condition
    pub fn matches(&self, cell_value: &str) -> bool {
        let value_lower = cell_value.to_lowercase();
        let filter_lower = self.value.to_lowercase();

        match self.operator {
            FilterOperator::Contains => value_lower.contains(&filter_lower),
            FilterOperator::Equals => value_lower == filter_lower,
            FilterOperator::StartsWith => value_lower.starts_with(&filter_lower),
            FilterOperator::EndsWith => value_lower.ends_with(&filter_lower),
            FilterOperator::GreaterThan => {
                // Try numeric comparison first, fall back to string comparison
                if let (Ok(cell_num), Ok(filter_num)) =
                    (cell_value.parse::<f64>(), self.value.parse::<f64>())
                {
                    cell_num > filter_num
                } else {
                    value_lower > filter_lower
                }
            }
            FilterOperator::LessThan => {
                if let (Ok(cell_num), Ok(filter_num)) =
                    (cell_value.parse::<f64>(), self.value.parse::<f64>())
                {
                    cell_num < filter_num
                } else {
                    value_lower < filter_lower
                }
            }
            FilterOperator::NotEmpty => !cell_value.trim().is_empty(),
            FilterOperator::Empty => cell_value.trim().is_empty(),
        }
    }
}

/// Filter state for the application
#[derive(Clone, Default)]
pub struct FilterState {
    /// Active filters: original_column_index -> FilterCondition
    pub filters: HashMap<usize, FilterCondition>,
    /// Column currently showing filter popup (original index)
    pub active_popup: Option<usize>,
    /// Input text for filter value (in popup)
    pub filter_input: String,
    /// Selected operator in popup
    pub selected_operator: FilterOperator,
}

impl FilterState {
    /// Check if any filters are active
    pub fn has_active_filters(&self) -> bool {
        !self.filters.is_empty()
    }

    /// Check if a specific column has an active filter
    pub fn has_filter(&self, column_idx: usize) -> bool {
        self.filters.contains_key(&column_idx)
    }

    /// Apply filter to a column
    pub fn apply_filter(&mut self, column_idx: usize, operator: FilterOperator, value: String) {
        self.filters
            .insert(column_idx, FilterCondition { operator, value });
        self.active_popup = None;
        self.filter_input.clear();
    }

    /// Clear filter for a column
    pub fn clear_filter(&mut self, column_idx: usize) {
        self.filters.remove(&column_idx);
    }

    /// Check if a row matches all active filters
    /// `row_data` is indexed by original column index
    pub fn row_matches(&self, row_data: &[String]) -> bool {
        for (&col_idx, condition) in &self.filters {
            if let Some(cell_value) = row_data.get(col_idx) {
                if !condition.matches(cell_value) {
                    return false;
                }
            } else {
                // Column doesn't exist in row, treat as empty
                if !condition.matches("") {
                    return false;
                }
            }
        }
        true
    }

    /// Open filter popup for a column
    pub fn open_popup(&mut self, column_idx: usize) {
        self.active_popup = Some(column_idx);
        // Pre-fill with existing filter if any
        if let Some(condition) = self.filters.get(&column_idx) {
            self.filter_input = condition.value.clone();
            self.selected_operator = condition.operator;
        } else {
            self.filter_input.clear();
            self.selected_operator = FilterOperator::default();
        }
    }

    /// Close filter popup
    pub fn close_popup(&mut self) {
        self.active_popup = None;
        self.filter_input.clear();
    }
}
