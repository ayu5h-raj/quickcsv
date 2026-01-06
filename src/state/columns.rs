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
