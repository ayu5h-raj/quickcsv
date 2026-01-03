# QuickCSV Architecture & Core Logic

This document explains the internal workings of QuickCSV. It is designed for developers who want to understand the high-performance techniques and Rust patterns used in this project.

---

## 1. High-Level Design

QuickCSV uses an **Immediate Mode GUI** (`egui`) and a **Multi-threaded Architecture**. 

- **UI Thread**: Handles rendering at 60 FPS. It must never perform blocking I/O or heavy computation.
- **Worker Threads**: Handle file indexing, searching, and sorting in the background to keep the UI responsive.

---

## 2. Zero-Copy Memory Mapping (`MappedCsv`)

Instead of loading the entire CSV into RAM, QuickCSV uses **Memory Mapping** via the `memmap2` crate.

- **How it works**: The operating system maps the file on disk to a virtual memory address space.
- **Benefit**: We can handle 2GB+ files while only using a few megabytes of RAM. The OS automatically manages loading "pages" of the file into physical memory only when we access specific byte ranges.

```rust
struct MappedCsv {
    mmap: Mmap,                // The memory-mapped file data
    row_offsets: Arc<RwLock<Vec<usize>>>, // Byte position of every row
    // ...
}
```

---

## 3. Progressive Loading & Indexing

QuickCSV doesn't make you wait for the whole file to be scanned.

1. **Header Phase**: The app immediately finds the first row boundary to extract column names.
2. **Ready State**: The app switches to `Ready` state and shows the first few rows instantly.
3. **Background Indexing**: A thread scans the rest of the file for row boundaries (`\n` or `\r\n`).
4. **Dynamic Update**: As the `row_offsets` list grows, the UI's scrollbar and row count update automatically.

---

## 4. RFC 4180 Compliant Parsing

Standard CSV parsing is tricky because fields can contain newlines if they are quoted.

- **`find_row_boundary`**: This function uses a small "State Machine" to track whether it is currently inside a quoted field. If it sees a newline (`\n`) while `in_quotes` is true, it treats it as regular text rather than a row ending.
- **Field Parsing**: We use the `csv` crate for field extraction, but we use our own `find_row_boundary` logic for row-level indexing to maintain performance.

---

## 5. Virtualized Rendering

Even with 10 million rows, the UI only ever "sees" the ~30 rows currently visible on your screen.

- **The Table**: `egui_extras::TableBuilder` calculates which row indices are visible based on the scroll position.
- **The Cache**: Parsing bytes into Strings is fast, but doing it every frame for every visible cell can be CPU-intensive. `FastCsvApp` maintains a `row_cache` (HashMap) of the most recently parsed rows to ensure smooth 60 FPS scrolling.

---

## 6. Search & Sort Logic

### Search
- **Background Scanning**: Search runs in a separate thread to avoid UI lag.
- **Navigation vs. Highlighting**: We only store the first 10,000 match locations for navigation (Prev/Next). Highlighting is done **on-the-fly** by the UI thread only for the rows currently visible in the viewport.

### Sorting
- **Indirect Sorting**: We never move data in the file. Instead, we create a `sorted_indices` vector.
- **Display Mapping**: When the UI asks for "Display Row 5", the app looks up `sorted_indices[5]` to find the "Actual Row" index in the memory map.

---

## 7. Key Rust Patterns

- **`Arc<RwLock<T>>`**: "Atomic Reference Count" + "Read-Write Lock". Used to share the CSV data safely between the UI and background workers.
- **`AtomicBool` / `AtomicUsize`**: High-performance, thread-safe primitives used for cancellation flags and progress counters without the overhead of a full Lock.
- **Option/Result**: Rust's way of handling missing data and errors without ever crashing with "NullPointerExceptions".

---

## 8. Summary of Performance Secrets

1. **Don't Copy**: Use `memmap2` to read directly from disk.
2. **Don't Wait**: Index files in the background.
3. **Don't Render Everything**: Only parse and draw what is visible.
4. **Don't Block**: Always move heavy work to a thread.
