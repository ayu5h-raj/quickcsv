//! Filesystem watcher for detecting external changes to open CSV files.

use eframe::egui;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime};

const RELOAD_DEBOUNCE: Duration = Duration::from_millis(500);
const SNAPSHOT_EVENT_WINDOW: Duration = Duration::from_secs(2);

fn is_content_change(event: &Event) -> bool {
    use notify::event::ModifyKind;
    match event.kind {
        EventKind::Create(_) | EventKind::Remove(_) => true,
        EventKind::Modify(ModifyKind::Data(_)) | EventKind::Modify(ModifyKind::Name(_)) => true,
        EventKind::Modify(ModifyKind::Any) => true,
        EventKind::Modify(ModifyKind::Metadata(_)) => false,
        _ => false,
    }
}

#[derive(Default)]
struct WatchedFile {
    aliases: HashMap<PathBuf, usize>,
    refs: usize,
    fingerprint: Option<FileFingerprint>,
}

#[derive(Clone, PartialEq)]
struct FileFingerprint {
    len: u64,
    modified: Option<SystemTime>,
    inode: u64,
    changed_at: (i64, i64),
}

fn file_fingerprint(path: &Path) -> Option<FileFingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    let (inode, changed_at) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.ino(), (metadata.ctime(), metadata.ctime_nsec()))
    };
    #[cfg(not(unix))]
    let inode = 0;
    #[cfg(not(unix))]
    let changed_at = (0, 0);
    Some(FileFingerprint {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        inode,
        changed_at,
    })
}

struct PendingChange {
    last_event_at: Instant,
}

/// Actions produced by one watcher poll.
pub struct FileChangeBatch {
    /// Paths whose currently mapped data must be hidden immediately.
    pub suspend: Vec<PathBuf>,
    /// Paths that have been quiet long enough to load safely.
    pub reload: Vec<PathBuf>,
    /// Delay until the next pending path reaches the trailing debounce edge.
    pub next_check: Option<Duration>,
}

/// Tracks open files, filters parent-directory noise, and trailing-debounces saves.
pub struct FileWatcher {
    watcher: RecommendedWatcher,
    rx: mpsc::Receiver<PathBuf>,
    #[cfg(test)]
    tx: mpsc::Sender<PathBuf>,
    watched: Arc<RwLock<HashMap<PathBuf, WatchedFile>>>,
    pending_paths: Arc<RwLock<HashSet<PathBuf>>>,
    snapshot_windows: Arc<RwLock<HashMap<PathBuf, Instant>>>,
    watched_dirs: HashMap<PathBuf, usize>,
    pending: HashMap<PathBuf, PendingChange>,
}

impl FileWatcher {
    pub fn new(ctx: egui::Context) -> Result<Self, notify::Error> {
        let (tx, rx) = mpsc::channel();
        let watched = Arc::new(RwLock::new(HashMap::<PathBuf, WatchedFile>::new()));
        let watched_for_callback = Arc::clone(&watched);
        let pending_paths = Arc::new(RwLock::new(HashSet::new()));
        let pending_for_callback = Arc::clone(&pending_paths);
        let snapshot_windows = Arc::new(RwLock::new(HashMap::<PathBuf, Instant>::new()));
        let snapshots_for_callback = Arc::clone(&snapshot_windows);
        let callback_tx = tx.clone();

        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else {
                return;
            };
            if !is_content_change(&event) {
                return;
            }

            let candidates: HashMap<PathBuf, Option<FileFingerprint>> = {
                let registry = watched_for_callback.read();
                let mut candidates = HashMap::new();
                for event_path in &event.paths {
                    let canonical = event_path.canonicalize().ok();
                    for (watched_path, file) in registry.iter() {
                        let is_match = canonical.as_ref() == Some(watched_path)
                            || event_path == watched_path
                            || file.aliases.contains_key(event_path);
                        if is_match {
                            candidates.insert(watched_path.clone(), file.fingerprint.clone());
                        }
                    }
                }
                candidates
            };

            if candidates.is_empty() {
                return;
            }
            let pending = pending_for_callback.read();
            let snapshot_windows = snapshots_for_callback.read();
            let event_time = Instant::now();
            let changed: Vec<(PathBuf, Option<FileFingerprint>)> = candidates
                .into_iter()
                .filter_map(|(path, previous)| {
                    let current = file_fingerprint(&path);
                    let suppress_unchanged = snapshot_windows
                        .get(&path)
                        .is_some_and(|deadline| event_time < *deadline);
                    (pending.contains(&path) || previous != current || !suppress_unchanged)
                        .then_some((path, current))
                })
                .collect();
            drop(snapshot_windows);
            drop(pending);
            if changed.is_empty() {
                return;
            }

            {
                let mut registry = watched_for_callback.write();
                for (path, fingerprint) in &changed {
                    if let Some(file) = registry.get_mut(path) {
                        file.fingerprint = fingerprint.clone();
                    }
                }
            }
            for (path, _) in changed {
                let _ = callback_tx.send(path);
            }
            ctx.request_repaint();
        })?;

        Ok(Self {
            watcher,
            rx,
            #[cfg(test)]
            tx,
            watched,
            pending_paths,
            snapshot_windows,
            watched_dirs: HashMap::new(),
            pending: HashMap::new(),
        })
    }

    /// Register one tab's interest in a file.
    pub fn register(&mut self, path: &Path) -> Option<PathBuf> {
        let Ok(canonical) = path.canonicalize() else {
            return None;
        };
        let alias = path.to_path_buf();

        {
            let mut watched = self.watched.write();
            if let Some(file) = watched.get_mut(&canonical) {
                file.refs += 1;
                *file.aliases.entry(alias).or_insert(0) += 1;
                return Some(canonical);
            }
        }

        let parent = canonical.parent().map(Path::to_path_buf)?;
        if !self.watched_dirs.contains_key(&parent)
            && self
                .watcher
                .watch(&parent, RecursiveMode::NonRecursive)
                .is_err()
        {
            return None;
        }

        *self.watched_dirs.entry(parent).or_insert(0) += 1;
        let mut aliases = HashMap::new();
        aliases.insert(alias, 1);
        let fingerprint = file_fingerprint(&canonical);
        self.watched.write().insert(
            canonical.clone(),
            WatchedFile {
                aliases,
                refs: 1,
                fingerprint,
            },
        );
        Some(canonical)
    }

    /// Remove one tab's interest and unwatch the directory after the last file closes.
    pub fn unregister(&mut self, canonical: &Path, alias: &Path) {
        let remove_file = {
            let mut watched = self.watched.write();
            let Some(file) = watched.get_mut(canonical) else {
                return;
            };
            file.refs = file.refs.saturating_sub(1);
            if let Some(alias_refs) = file.aliases.get_mut(alias) {
                *alias_refs = alias_refs.saturating_sub(1);
                if *alias_refs == 0 {
                    file.aliases.remove(alias);
                }
            }
            if file.refs == 0 {
                watched.remove(canonical);
                true
            } else {
                false
            }
        };
        if !remove_file {
            return;
        }

        self.pending.remove(canonical);
        self.pending_paths.write().remove(canonical);
        self.snapshot_windows.write().remove(canonical);
        let Some(parent) = canonical.parent().map(Path::to_path_buf) else {
            return;
        };
        let remove_dir = match self.watched_dirs.get_mut(&parent) {
            Some(file_count) => {
                *file_count = file_count.saturating_sub(1);
                *file_count == 0
            }
            None => false,
        };
        if remove_dir {
            self.watched_dirs.remove(&parent);
            let _ = self.watcher.unwatch(&parent);
        }
    }

    /// Ignore fingerprint-stable FSEvents caused by QuickCSV's own COW snapshot.
    pub fn begin_snapshot(&mut self, path: &Path, now: Instant) {
        let canonical = path.canonicalize().ok().or_else(|| {
            self.watched
                .read()
                .iter()
                .find(|(_, file)| file.aliases.contains_key(path))
                .map(|(canonical, _)| canonical.clone())
        });
        if let Some(canonical) = canonical {
            self.snapshot_windows
                .write()
                .insert(canonical, now + SNAPSHOT_EVENT_WINDOW);
        }
    }

    /// Drain new events and emit immediate suspend plus trailing-edge reload actions.
    pub fn poll(&mut self, now: Instant) -> FileChangeBatch {
        let mut suspend_keys = HashSet::new();
        while let Ok(canonical) = self.rx.try_recv() {
            if !self.watched.read().contains_key(&canonical) {
                continue;
            }
            if !self.pending.contains_key(&canonical) {
                suspend_keys.insert(canonical.clone());
            }
            self.pending_paths.write().insert(canonical.clone());
            self.pending
                .insert(canonical, PendingChange { last_event_at: now });
        }

        let reload_keys: Vec<PathBuf> = self
            .pending
            .iter()
            .filter(|(_, pending)| now.duration_since(pending.last_event_at) >= RELOAD_DEBOUNCE)
            .map(|(path, _)| path.clone())
            .collect();
        for path in &reload_keys {
            self.pending.remove(path);
            self.pending_paths.write().remove(path);
        }

        let next_check = self
            .pending
            .values()
            .map(|pending| {
                RELOAD_DEBOUNCE.saturating_sub(now.duration_since(pending.last_event_at))
            })
            .min();
        let watched = self.watched.read();
        let expand_aliases = |keys: &HashSet<PathBuf>| {
            keys.iter()
                .filter_map(|key| watched.get(key))
                .flat_map(|file| file.aliases.keys().cloned())
                .collect()
        };
        let reload_key_set: HashSet<PathBuf> = reload_keys.into_iter().collect();

        FileChangeBatch {
            suspend: expand_aliases(&suspend_keys),
            reload: expand_aliases(&reload_key_set),
            next_check,
        }
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

    fn temp_csv(name: &str) -> PathBuf {
        let unique = format!(
            "quickcsv-watcher-{}-{name}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after unix epoch")
                .as_nanos()
        );
        std::env::temp_dir().join(unique)
    }

    #[test]
    fn recognizes_only_content_changes() {
        assert!(is_content_change(&event(EventKind::Modify(
            ModifyKind::Data(DataChange::Content)
        ))));
        assert!(is_content_change(&event(EventKind::Modify(
            ModifyKind::Any
        ))));
        assert!(is_content_change(&event(EventKind::Create(
            CreateKind::File
        ))));
        assert!(is_content_change(&event(EventKind::Remove(
            RemoveKind::File
        ))));
        assert!(is_content_change(&event(EventKind::Modify(
            ModifyKind::Name(notify::event::RenameMode::Any)
        ))));
        assert!(!is_content_change(&event(EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::Any)
        ))));
        assert!(!is_content_change(&event(EventKind::Access(
            AccessKind::Read
        ))));
    }

    #[test]
    fn same_file_registration_is_reference_counted() {
        let path = temp_csv("refs.csv");
        std::fs::write(&path, "id\n1\n").expect("should create temp csv");
        let mut watcher = FileWatcher::new(egui::Context::default()).expect("watcher should start");

        let first = watcher
            .register(&path)
            .expect("first watch should register");
        let second = watcher
            .register(&path)
            .expect("second watch should register");
        assert_eq!(first, second);
        watcher.unregister(&first, &path);
        assert_eq!(watcher.watched.read().len(), 1);
        watcher.unregister(&second, &path);
        assert!(watcher.watched.read().is_empty());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reload_is_emitted_only_after_quiet_period() {
        let path = temp_csv("debounce.csv");
        std::fs::write(&path, "id\n1\n").expect("should create temp csv");
        let canonical = path.canonicalize().expect("temp csv should canonicalize");
        let mut watcher = FileWatcher::new(egui::Context::default()).expect("watcher should start");
        assert!(watcher.register(&path).is_some());

        let start = Instant::now();
        watcher
            .tx
            .send(canonical.clone())
            .expect("event should queue");
        let first = watcher.poll(start);
        assert_eq!(first.suspend, vec![path.clone()]);
        assert!(first.reload.is_empty());

        watcher
            .tx
            .send(canonical)
            .expect("second event should queue");
        let second = watcher.poll(start + Duration::from_millis(300));
        assert!(second.suspend.is_empty());
        assert!(second.reload.is_empty());

        let early = watcher.poll(start + Duration::from_millis(799));
        assert!(early.reload.is_empty());
        let settled = watcher.poll(start + Duration::from_millis(800));
        assert_eq!(settled.reload, vec![path.clone()]);

        let _ = std::fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_retarget_unregisters_the_original_registration() {
        use std::os::unix::fs::symlink;

        let first_target = temp_csv("first-target.csv");
        let second_target = temp_csv("second-target.csv");
        let alias = temp_csv("alias.csv");
        std::fs::write(&first_target, "id\n1\n").expect("should create first target");
        std::fs::write(&second_target, "id\n2\n").expect("should create second target");
        symlink(&first_target, &alias).expect("should create symlink");

        let mut watcher = FileWatcher::new(egui::Context::default()).expect("watcher should start");
        let first_registration = watcher
            .register(&alias)
            .expect("first target should register");
        std::fs::remove_file(&alias).expect("should remove old symlink");
        symlink(&second_target, &alias).expect("should retarget symlink");
        let second_registration = watcher
            .register(&alias)
            .expect("second target should register");

        watcher.unregister(&first_registration, &alias);
        assert!(!watcher.watched.read().contains_key(&first_registration));
        assert!(watcher.watched.read().contains_key(&second_registration));

        watcher.unregister(&second_registration, &alias);
        let _ = std::fs::remove_file(alias);
        let _ = std::fs::remove_file(first_target);
        let _ = std::fs::remove_file(second_target);
    }
}
