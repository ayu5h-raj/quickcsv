//! Filter state management for column filtering

use std::collections::HashMap;

/// Filter operators for column filtering
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilterOperator {
    #[default]
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

    /// Clear all filters
    pub fn clear(&mut self) {
        self.filters.clear();
        self.active_popup = None;
        self.filter_input.clear();
    }

    /// Close filter popup
    pub fn close_popup(&mut self) {
        self.active_popup = None;
        self.filter_input.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // FilterCondition::matches tests
    #[test]
    fn test_contains_operator() {
        let condition = FilterCondition {
            operator: FilterOperator::Contains,
            value: "hello".to_string(),
        };
        assert!(condition.matches("Hello World"));
        assert!(condition.matches("say hello"));
        assert!(!condition.matches("Hi World"));
    }

    #[test]
    fn test_equals_operator() {
        let condition = FilterCondition {
            operator: FilterOperator::Equals,
            value: "test".to_string(),
        };
        assert!(condition.matches("Test"));
        assert!(condition.matches("TEST"));
        assert!(!condition.matches("testing"));
    }

    #[test]
    fn test_starts_with_operator() {
        let condition = FilterCondition {
            operator: FilterOperator::StartsWith,
            value: "hello".to_string(),
        };
        assert!(condition.matches("Hello World"));
        assert!(!condition.matches("Say Hello"));
    }

    #[test]
    fn test_ends_with_operator() {
        let condition = FilterCondition {
            operator: FilterOperator::EndsWith,
            value: "world".to_string(),
        };
        assert!(condition.matches("Hello World"));
        assert!(!condition.matches("World Hello"));
    }

    #[test]
    fn test_greater_than_numeric() {
        let condition = FilterCondition {
            operator: FilterOperator::GreaterThan,
            value: "100".to_string(),
        };
        assert!(condition.matches("150"));
        assert!(condition.matches("100.5"));
        assert!(!condition.matches("50"));
        assert!(!condition.matches("100"));
    }

    #[test]
    fn test_less_than_numeric() {
        let condition = FilterCondition {
            operator: FilterOperator::LessThan,
            value: "100".to_string(),
        };
        assert!(condition.matches("50"));
        assert!(condition.matches("99.9"));
        assert!(!condition.matches("150"));
        assert!(!condition.matches("100"));
    }

    #[test]
    fn test_greater_than_string_fallback() {
        let condition = FilterCondition {
            operator: FilterOperator::GreaterThan,
            value: "apple".to_string(),
        };
        assert!(condition.matches("banana"));
        assert!(!condition.matches("aardvark"));
    }

    #[test]
    fn test_empty_operator() {
        let condition = FilterCondition {
            operator: FilterOperator::Empty,
            value: String::new(),
        };
        assert!(condition.matches(""));
        assert!(condition.matches("   "));
        assert!(!condition.matches("text"));
    }

    #[test]
    fn test_not_empty_operator() {
        let condition = FilterCondition {
            operator: FilterOperator::NotEmpty,
            value: String::new(),
        };
        assert!(condition.matches("text"));
        assert!(!condition.matches(""));
        assert!(!condition.matches("   "));
    }

    // FilterState tests
    #[test]
    fn test_has_active_filters() {
        let mut state = FilterState::default();
        assert!(!state.has_active_filters());

        state.apply_filter(0, FilterOperator::Contains, "test".to_string());
        assert!(state.has_active_filters());

        state.clear_filter(0);
        assert!(!state.has_active_filters());
    }

    #[test]
    fn test_row_matches_single_filter() {
        let mut state = FilterState::default();
        state.apply_filter(1, FilterOperator::Equals, "Engineering".to_string());

        let row1 = vec![
            "1".to_string(),
            "Engineering".to_string(),
            "Alice".to_string(),
        ];
        let row2 = vec!["2".to_string(), "Sales".to_string(), "Bob".to_string()];

        assert!(state.row_matches(&row1));
        assert!(!state.row_matches(&row2));
    }

    #[test]
    fn test_row_matches_multiple_filters() {
        let mut state = FilterState::default();
        state.apply_filter(1, FilterOperator::Equals, "Engineering".to_string());
        state.apply_filter(2, FilterOperator::GreaterThan, "50000".to_string());

        let row1 = vec![
            "1".to_string(),
            "Engineering".to_string(),
            "75000".to_string(),
        ];
        let row2 = vec![
            "2".to_string(),
            "Engineering".to_string(),
            "40000".to_string(),
        ];
        let row3 = vec!["3".to_string(), "Sales".to_string(), "75000".to_string()];

        assert!(state.row_matches(&row1)); // Both match
        assert!(!state.row_matches(&row2)); // Salary too low
        assert!(!state.row_matches(&row3)); // Wrong dept
    }

    #[test]
    fn test_row_matches_missing_column() {
        let mut state = FilterState::default();
        state.apply_filter(5, FilterOperator::Empty, String::new());

        // Row only has 3 columns, filter is on column 5
        let row = vec!["1".to_string(), "Test".to_string(), "Value".to_string()];

        // Missing column treated as empty, Empty filter should match
        assert!(state.row_matches(&row));
    }
}
