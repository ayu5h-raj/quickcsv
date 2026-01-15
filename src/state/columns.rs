//! Column state management for hiding, showing, and reordering columns

use std::collections::HashSet;

/// Action types for undo/redo
#[derive(Clone, Debug)]
pub enum ColumnAction {
    /// Hide a column
    HideColumn(usize),
    /// Show a column
    ShowColumn(usize),
    /// Reorder columns (from_index, to_index)
    ReorderColumns { from: usize, to: usize },
    /// Batch action (multiple operations)
    Batch(Vec<ColumnAction>),
}

/// Column state management
#[derive(Clone, Default)]
pub struct ColumnState {
    /// Set of hidden column indices (original indices)
    pub hidden_columns: HashSet<usize>,
    /// Column order mapping: display_index -> original_index
    /// Example: [2, 0, 1] means display col 0 shows original col 2, etc.
    pub column_order: Vec<usize>,
    /// Undo stack (stores actions that can be undone)
    pub undo_stack: Vec<ColumnAction>,
    /// Redo stack (stores actions that can be redone)
    pub redo_stack: Vec<ColumnAction>,
    /// Whether the column manager dialog is open
    pub manager_open: bool,
    /// Column being dragged (visible index)
    pub dragged_column: Option<usize>,
}

impl ColumnState {
    /// Initialize column order for a given number of columns
    pub fn init_column_order(&mut self, num_columns: usize) {
        if self.column_order.len() != num_columns {
            self.column_order = (0..num_columns).collect();
            self.hidden_columns.clear(); // Reset hidden columns when switching files
        }
    }

    /// Get visible columns in display order
    pub fn get_visible_columns(&self) -> Vec<usize> {
        self.column_order
            .iter()
            .filter(|&&idx| !self.hidden_columns.contains(&idx))
            .copied()
            .collect()
    }

    /// Hide a column by original index
    pub fn hide_column(&mut self, original_idx: usize) {
        // Don't allow hiding all columns
        if self.hidden_columns.len() + 1 < self.column_order.len() {
            self.hidden_columns.insert(original_idx);
            self.push_undo(ColumnAction::HideColumn(original_idx));
            self.clear_redo();
        }
    }

    /// Show a column by original index
    pub fn show_column(&mut self, original_idx: usize) {
        if self.hidden_columns.remove(&original_idx) {
            self.push_undo(ColumnAction::ShowColumn(original_idx));
            self.clear_redo();
        }
    }

    /// Toggle column visibility
    pub fn toggle_column(&mut self, original_idx: usize) {
        if self.hidden_columns.contains(&original_idx) {
            self.show_column(original_idx);
        } else {
            self.hide_column(original_idx);
        }
    }

    /// Reorder columns (move from display index to another display index)
    /// This works on ALL columns including hidden ones
    pub fn reorder_columns(&mut self, from_display: usize, to_display: usize) {
        if from_display >= self.column_order.len() || to_display >= self.column_order.len() {
            return;
        }

        let original_from = self.column_order[from_display];
        let _original_to = self.column_order[to_display];

        // Move the column
        self.column_order.remove(from_display);
        self.column_order.insert(to_display, original_from);

        self.push_undo(ColumnAction::ReorderColumns {
            from: from_display,
            to: to_display,
        });
        self.clear_redo();
    }

    /// Reorder visible columns only (skips hidden columns)
    /// Moves a visible column to another visible position
    pub fn reorder_visible_columns(&mut self, from_visible_idx: usize, to_visible_idx: usize) {
        let visible = self.get_visible_columns();
        if from_visible_idx >= visible.len() || to_visible_idx >= visible.len() {
            return;
        }

        let original_from = visible[from_visible_idx];
        let original_to = visible[to_visible_idx];

        // Find display indices for these original indices
        let from_display = self
            .column_order
            .iter()
            .position(|&idx| idx == original_from)
            .unwrap_or(from_visible_idx);
        let to_display = self
            .column_order
            .iter()
            .position(|&idx| idx == original_to)
            .unwrap_or(to_visible_idx);

        // Reorder in the full column_order array
        self.reorder_columns(from_display, to_display);
    }

    /// Show all columns
    pub fn show_all_columns(&mut self) {
        if !self.hidden_columns.is_empty() {
            let hidden: Vec<usize> = self.hidden_columns.iter().copied().collect();
            self.hidden_columns.clear();
            self.push_undo(ColumnAction::Batch(
                hidden
                    .iter()
                    .map(|&idx| ColumnAction::ShowColumn(idx))
                    .collect(),
            ));
            self.clear_redo();
        }
    }

    /// Hide all columns except the first one (must keep at least one visible)
    pub fn hide_all_columns(&mut self) {
        let total_columns = self.column_order.len();
        if total_columns <= 1 {
            return; // Nothing to hide
        }

        // Get columns that are currently visible (excluding already hidden)
        let visible: Vec<usize> = self
            .column_order
            .iter()
            .filter(|&&idx| !self.hidden_columns.contains(&idx))
            .copied()
            .collect();

        if visible.len() <= 1 {
            return; // Already only one visible
        }

        // Hide all except the first visible column
        let to_hide: Vec<usize> = visible.into_iter().skip(1).collect();
        for &idx in &to_hide {
            self.hidden_columns.insert(idx);
        }

        self.push_undo(ColumnAction::Batch(
            to_hide
                .iter()
                .map(|&idx| ColumnAction::HideColumn(idx))
                .collect(),
        ));
        self.clear_redo();
    }

    /// Reset column order and visibility
    #[allow(dead_code)]
    pub fn reset_columns(&mut self) {
        self.show_all_columns();
        self.reset_column_order();
    }

    /// Reset column order to original
    pub fn reset_column_order(&mut self) {
        if self
            .column_order
            .iter()
            .enumerate()
            .any(|(i, &idx)| i != idx)
        {
            let _old_order = self.column_order.clone();
            self.column_order = (0..self.column_order.len()).collect();
            // Store undo action (would need to track old order for proper undo)
            self.clear_redo();
        }
    }

    /// Push action to undo stack
    fn push_undo(&mut self, action: ColumnAction) {
        self.undo_stack.push(action);
        // Limit undo stack size
        if self.undo_stack.len() > 50 {
            self.undo_stack.remove(0);
        }
    }

    /// Clear redo stack
    fn clear_redo(&mut self) {
        self.redo_stack.clear();
    }

    /// Undo last action
    pub fn undo(&mut self) -> bool {
        if let Some(action) = self.undo_stack.pop() {
            match action {
                ColumnAction::HideColumn(idx) => {
                    self.hidden_columns.remove(&idx);
                    self.redo_stack.push(ColumnAction::HideColumn(idx));
                }
                ColumnAction::ShowColumn(idx) => {
                    self.hidden_columns.insert(idx);
                    self.redo_stack.push(ColumnAction::ShowColumn(idx));
                }
                ColumnAction::ReorderColumns { from, to } => {
                    // Reverse the reorder
                    self.reorder_columns(to, from);
                    self.undo_stack.pop(); // Remove the action we just created
                    self.redo_stack
                        .push(ColumnAction::ReorderColumns { from, to });
                }
                ColumnAction::Batch(actions) => {
                    // Reverse all actions in batch
                    for action in actions.iter().rev() {
                        match action {
                            ColumnAction::HideColumn(idx) => {
                                self.hidden_columns.remove(idx);
                            }
                            ColumnAction::ShowColumn(idx) => {
                                self.hidden_columns.insert(*idx);
                            }
                            _ => {}
                        }
                    }
                    self.redo_stack.push(ColumnAction::Batch(actions));
                }
            }
            true
        } else {
            false
        }
    }

    /// Redo last undone action
    pub fn redo(&mut self) -> bool {
        if let Some(action) = self.redo_stack.pop() {
            match action {
                ColumnAction::HideColumn(idx) => {
                    self.hide_column(idx);
                    self.undo_stack.pop(); // Remove the action we just created
                }
                ColumnAction::ShowColumn(idx) => {
                    self.show_column(idx);
                    self.undo_stack.pop(); // Remove the action we just created
                }
                ColumnAction::ReorderColumns { from, to } => {
                    self.reorder_columns(from, to);
                    self.undo_stack.pop(); // Remove the action we just created
                }
                ColumnAction::Batch(actions) => {
                    for action in actions {
                        match action {
                            ColumnAction::HideColumn(idx) => {
                                self.hidden_columns.insert(idx);
                            }
                            ColumnAction::ShowColumn(idx) => {
                                self.hidden_columns.remove(&idx);
                            }
                            _ => {}
                        }
                    }
                    self.undo_stack.pop(); // Remove the action we just created
                }
            }
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_column_order() {
        let mut state = ColumnState::default();
        state.init_column_order(5);

        assert_eq!(state.column_order, vec![0, 1, 2, 3, 4]);
        assert_eq!(state.hidden_columns.len(), 0);
    }

    #[test]
    fn test_init_column_order_clears_hidden_columns() {
        let mut state = ColumnState::default();
        state.init_column_order(5);

        // Hide some columns
        state.hide_column(1);
        state.hide_column(3);
        assert_eq!(state.hidden_columns.len(), 2);

        // Initialize with different column count - should clear hidden columns
        state.init_column_order(3);
        assert_eq!(state.column_order, vec![0, 1, 2]);
        assert_eq!(
            state.hidden_columns.len(),
            0,
            "Hidden columns should be cleared when column count changes"
        );
    }

    #[test]
    fn test_init_column_order_preserves_state_if_same_count() {
        let mut state = ColumnState::default();
        state.init_column_order(5);
        state.hide_column(1);
        state.hide_column(3);

        // Initialize with same column count - should preserve hidden columns
        state.init_column_order(5);
        assert_eq!(state.column_order, vec![0, 1, 2, 3, 4]);
        assert_eq!(
            state.hidden_columns.len(),
            2,
            "Hidden columns should be preserved when column count stays the same"
        );
    }

    #[test]
    fn test_hide_column() {
        let mut state = ColumnState::default();
        state.init_column_order(5);

        state.hide_column(2);
        assert!(state.hidden_columns.contains(&2));
        assert_eq!(state.get_visible_columns(), vec![0, 1, 3, 4]);
    }

    #[test]
    fn test_show_column() {
        let mut state = ColumnState::default();
        state.init_column_order(5);
        state.hide_column(2);

        state.show_column(2);
        assert!(!state.hidden_columns.contains(&2));
        assert_eq!(state.get_visible_columns(), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_cannot_hide_all_columns() {
        let mut state = ColumnState::default();
        state.init_column_order(3);

        state.hide_column(0);
        state.hide_column(1);
        // Try to hide the last column - should fail
        state.hide_column(2);

        // At least one column must remain visible
        assert_eq!(
            state.get_visible_columns().len(),
            1,
            "Cannot hide all columns - at least one must remain visible"
        );
    }

    #[test]
    fn test_toggle_column() {
        let mut state = ColumnState::default();
        state.init_column_order(5);

        // Toggle to hide
        state.toggle_column(2);
        assert!(state.hidden_columns.contains(&2));

        // Toggle to show
        state.toggle_column(2);
        assert!(!state.hidden_columns.contains(&2));
    }

    #[test]
    fn test_get_visible_columns() {
        let mut state = ColumnState::default();
        state.init_column_order(5);

        state.hide_column(1);
        state.hide_column(3);

        let visible = state.get_visible_columns();
        assert_eq!(visible, vec![0, 2, 4]);
        assert!(!visible.contains(&1));
        assert!(!visible.contains(&3));
    }

    #[test]
    fn test_show_all_columns() {
        let mut state = ColumnState::default();
        state.init_column_order(5);

        state.hide_column(1);
        state.hide_column(2);
        state.hide_column(3);

        state.show_all_columns();
        assert_eq!(state.hidden_columns.len(), 0);
        assert_eq!(state.get_visible_columns(), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_hide_all_columns() {
        let mut state = ColumnState::default();
        state.init_column_order(5);

        state.hide_all_columns();
        // Should hide all except the first visible one
        assert_eq!(state.get_visible_columns().len(), 1);
    }

    #[test]
    fn test_reorder_columns() {
        let mut state = ColumnState::default();
        state.init_column_order(5);

        // Move column at index 0 to index 2
        state.reorder_columns(0, 2);
        assert_eq!(state.column_order, vec![1, 2, 0, 3, 4]);
    }

    #[test]
    fn test_reorder_visible_columns() {
        let mut state = ColumnState::default();
        state.init_column_order(5);
        state.hide_column(2); // Hide middle column

        // Visible columns are [0, 1, 3, 4] (indices into original)
        // Move visible column 0 (original 0) to visible position 2 (before original 4)
        state.reorder_visible_columns(0, 2);

        let visible = state.get_visible_columns();
        assert_eq!(visible[2], 0, "Column 0 should be at visible position 2");
    }

    #[test]
    fn test_undo_hide_column() {
        let mut state = ColumnState::default();
        state.init_column_order(5);

        state.hide_column(2);
        assert!(state.hidden_columns.contains(&2));

        state.undo();
        assert!(!state.hidden_columns.contains(&2));
    }

    #[test]
    fn test_undo_show_column() {
        let mut state = ColumnState::default();
        state.init_column_order(5);
        state.hide_column(2);

        state.show_column(2);
        assert!(!state.hidden_columns.contains(&2));

        state.undo();
        assert!(state.hidden_columns.contains(&2));
    }

    #[test]
    fn test_redo_hide_column() {
        let mut state = ColumnState::default();
        state.init_column_order(5);

        state.hide_column(2);
        state.undo();
        assert!(!state.hidden_columns.contains(&2));

        state.redo();
        assert!(state.hidden_columns.contains(&2));
    }

    #[test]
    fn test_undo_redo_reorder() {
        let mut state = ColumnState::default();
        state.init_column_order(5);

        let original_order = state.column_order.clone();
        state.reorder_columns(0, 2);
        let reordered = state.column_order.clone();
        assert_ne!(original_order, reordered);

        state.undo();
        assert_eq!(state.column_order, original_order);

        state.redo();
        assert_eq!(state.column_order, reordered);
    }

    #[test]
    fn test_reset_column_order() {
        let mut state = ColumnState::default();
        state.init_column_order(5);

        state.reorder_columns(0, 2);
        state.reorder_columns(1, 3);
        assert_ne!(state.column_order, vec![0, 1, 2, 3, 4]);

        state.reset_column_order();
        assert_eq!(state.column_order, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_batch_undo_show_all() {
        let mut state = ColumnState::default();
        state.init_column_order(5);

        state.hide_column(1);
        state.hide_column(2);
        state.hide_column(3);

        state.show_all_columns();
        assert_eq!(state.hidden_columns.len(), 0);

        // Undo the batch show all operation
        state.undo();
        assert_eq!(state.hidden_columns.len(), 3);
    }

    #[test]
    fn test_batch_undo_hide_all() {
        let mut state = ColumnState::default();
        state.init_column_order(5);

        state.hide_all_columns();
        let num_hidden = state.hidden_columns.len();
        assert!(num_hidden > 0);

        // Undo the batch hide all operation
        state.undo();
        assert_eq!(state.hidden_columns.len(), 0);
    }
}
