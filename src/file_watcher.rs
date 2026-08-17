//! Filesystem watcher for detecting external changes to open CSV files.
//!
//! Native only - web builds load files into browser memory and cannot watch
//! the filesystem, so auto-reload is a desktop-only feature.

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;

/// Minimum time between two auto-reloads of the same file (debounce).
///
/// Editors frequently emit several events for a single save (truncate, write,
/// metadata, rename), so we collapse them into one reload.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(500);

/// Whether a filesystem event indicates the file's contents may have changed.
///
/// Pure function so it can be unit-tested without a live watcher.
fn is_content_change(event: &Event) -> bool {
    use notify::event::ModifyKind;
    match event.kind {
        EventKind::Create(_) | EventKind::Remove(_) => true,
        EventKind::Modify(ModifyKind::Data(_)) | EventKind::Modify(ModifyKind::Name(_)) => true,
        EventKind::Modify(ModifyKind::Any) => true,
        // Metadata-only changes (mtime/chmod) are not content changes.
        EventKind::Modify(ModifyKind::Metadata(_)) => false,
        _ => false,
    }
}

/// Tracks open files and reports which ones changed on disk.
///
/// Watches the parent directory of each open file (rather than the file
/// itself) so atomic saves - where an editor replaces the file via rename -
/// are still detected. Events are filtered to the files we actually care about
/// and debounced to avoid duplicate reloads.
pub struct FileWatcher {
    /// The notify watcher driving events (kept alive for the struct's lifetime).
    watcher: RecommendedWatcher,
    /// Receives raw filesystem events from the notify thread.
    rx: mpsc::Receiver<notify::Result<Event>>,
    /// Canonical file path -> the original path it was registered under.
    watched: HashMap<PathBuf, PathBuf>,
    /// Timestamp of the last reported change per file (for debouncing).
    last_reported: HashMap<PathBuf, Instant>,
}

impl FileWatcher {
    /// Create a watcher. Events request a repaint on `ctx` so the UI thread
    /// wakes up and can drain the queued changes.
    pub fn new(ctx: egui::Context) -> Result<Self, notify::Error> {
        let (tx, rx) = mpsc::channel();
        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            ctx.request_repaint();
            let _ = tx.send(res);
        })?;
        Ok(Self {
            watcher,
            rx,
            watched: HashMap::new(),
            last_reported: HashMap::new(),
        })
    }

    /// Start watching `path` (by registering its parent directory).
    pub fn register(&mut self, path: &Path) {
        let Ok(canonical) = path.canonicalize() else {
            return;
        };
        if self.watched.contains_key(&canonical) {
            return;
        }
        let original = path.to_path_buf();
        self.watched.insert(canonical.clone(), original);

        let Some(parent) = canonical.parent() else {
            return;
        };
        // Best-effort: ignore errors (e.g. directory disappearing).
        let _ = self.watcher.watch(parent, RecursiveMode::NonRecursive);
    }

    /// Stop watching the file at `path`, unwatching its parent directory when
    /// no other registered file lives there anymore.
    pub fn unregister(&mut self, path: &Path) {
        let canonical = match path.canonicalize() {
            Ok(c) => c,
            // File may already be gone; match by the original registered path.
            Err(_) => {
                let key = self
                    .watched
                    .iter()
                    .find(|(_, original)| original.as_path() == path)
                    .map(|(canonical, _)| canonical.clone());
                match key {
                    Some(key) => key,
                    None => return,
                }
            }
        };

        if self.watched.remove(&canonical).is_none() {
            return;
        }
        self.last_reported.remove(&canonical);

        let Some(parent) = canonical.parent() else {
            return;
        };
        let still_used = self.watched.keys().any(|p| p.parent() == Some(parent));
        if !still_used {
            let _ = self.watcher.unwatch(parent);
        }
    }

    /// Drain pending events and return the canonical paths of files that
    /// changed on disk and are currently registered.
    pub fn changed_files(&mut self, now: Instant) -> Vec<PathBuf> {
        let mut changed = Vec::new();
        while let Ok(res) = self.rx.try_recv() {
            let Ok(event) = res else {
                continue;
            };
            if !is_content_change(&event) {
                continue;
            }
            for path in event.paths {
                let Ok(canonical) = path.canonicalize() else {
                    // File no longer exists - keep showing the last known data.
                    continue;
                };
                if !self.watched.contains_key(&canonical) {
                    continue;
                }
                let debounced = self
                    .last_reported
                    .get(&canonical)
                    .is_some_and(|t| now.duration_since(*t) < RELOAD_DEBOUNCE);
                if debounced {
                    continue;
                }
                self.last_reported.insert(canonical.clone(), now);
                if !changed.contains(&canonical) {
                    changed.push(canonical);
                }
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind};

    fn event(kind: EventKind) -> Event {
        Event {
            kind,
            paths: vec![PathBuf::from("/tmp/example.csv")],
            attrs: notify::event::EventAttributes::default(),
        }
    }

    #[test]
    fn data_modifications_are_content_changes() {
        assert!(is_content_change(&event(EventKind::Modify(
            ModifyKind::Data(DataChange::Any)
        ))));
        assert!(is_content_change(&event(EventKind::Modify(
            ModifyKind::Data(DataChange::Content)
        ))));
        assert!(is_content_change(&event(EventKind::Modify(
            ModifyKind::Any
        ))));
    }

    #[test]
    fn create_remove_and_rename_are_content_changes() {
        assert!(is_content_change(&event(EventKind::Create(
            CreateKind::File
        ))));
        assert!(is_content_change(&event(EventKind::Remove(
            RemoveKind::File
        ))));
        assert!(is_content_change(&event(EventKind::Modify(
            ModifyKind::Name(notify::event::RenameMode::Any)
        ))));
    }

    #[test]
    fn metadata_and_access_events_are_not_content_changes() {
        assert!(!is_content_change(&event(EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::Any)
        ))));
        assert!(!is_content_change(&event(EventKind::Access(
            AccessKind::Read
        ))));
        assert!(!is_content_change(&event(EventKind::Other)));
    }
}
