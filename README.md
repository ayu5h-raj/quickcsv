# QuickCSV

A blazing-fast CSV viewer for macOS that handles files from 100MB to 2GB+ with zero lag.

![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)
![Platform](https://img.shields.io/badge/platform-macOS-blue.svg)
![License](https://img.shields.io/badge/license-MIT-green.svg)

🌐 **Try it in your browser:** [https://ayu5h-raj.github.io/quickcsv/](https://ayu5h-raj.github.io/quickcsv/) *(Note: Web version loads files into RAM, unlike desktop which uses memory-mapped I/O)*

## Features

- 🚀 **Instant File Opening** - Memory-mapped I/O means no waiting
- 📊 **Handle Massive Files** - 100MB to 2GB+ with smooth scrolling
- ⚡ **Virtualized Table** - Only visible rows are rendered
- 🔍 **Fast Search** - Search millions of rows without freezing
- � **Auto-Detect Delimiter** - Supports CSV, TSV, semicolon, and pipe-delimited files
- �📄 **JSON Viewer** - Double-click JSON cells to view formatted content
- 🔢 **Row Numbers** - See row numbers at a glance
- 📋 **Row Inspector** - Double-click row numbers to view full details (Copy as JSON/CSV)
- 🎯 **Go to Row** - Jump to any row instantly (⌘L)
- 🔄 **Update Notifications** - Know when a new version is available
- 🎨 **Dark/Light Mode** - Toggle between themes
- 📁 **Drag & Drop** - Just drop your CSV file

## Installation

### Homebrew (Recommended)

```bash
brew tap ayu5h-raj/tap
brew install --cask quickcsv
```

**Check installed version:**
```bash
brew info --cask quickcsv
```

**Upgrade to latest version:**
```bash
brew update && brew upgrade --cask quickcsv
```

> **Note:** `brew update` is required first to fetch the latest cask definitions from the tap.

### Manual Download

Download the latest `QuickCSV.app` from the [Releases page](https://github.com/ayu5h-raj/quickcsv/releases).

### Try Web Version

You can try QuickCSV in your browser at **[https://ayu5h-raj.github.io/quickcsv/](https://ayu5h-raj.github.io/quickcsv/)**.

> **Note:** The web version is not as efficient as the desktop version due to browser limitations, but it's great for trying out QuickCSV without installation. On a MacBook M2 Air, files typically open in a few seconds.

## Build from Source

```bash
# Clone the repo
git clone https://github.com/ayu5h-raj/quickcsv.git
cd quickcsv

# Enable pre-commit hook (recommended)
git config core.hooksPath .githooks

# Build and run
cargo run --release
```

### Create macOS App Bundle

```bash
cargo install cargo-bundle  # one time
cargo bundle --release
# Output: target/release/bundle/osx/QuickCSV.app
```

## How It Works

QuickCSV uses memory-mapped I/O to handle large files efficiently:

1. **Memory Mapping** - File is mapped directly to memory (no loading into RAM)
2. **Background Indexing** - Row offsets are indexed in a background thread
3. **Virtualized Rendering** - Only visible rows are parsed and rendered
4. **Row Caching** - Recently viewed rows are cached for smooth scrolling

## Performance

Tested on MacBook Air M2. File opens **instantly** — indexing happens in the background.

| File Size | Rows | Open Time | Index Time | Scroll |
|-----------|------|-----------|------------|--------|
| 10 MB | 100K | Instant | ~50ms | Smooth |
| 100 MB | 740K | Instant | ~200ms | Smooth |
| 1 GB | 7M+ | Instant | ~1s | Smooth |
| 3 GB | 20M+ | Instant | ~3s | Smooth |

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `⌘O` | Open file |
| `⌘F` | Find/Search |
| `⌘L` | Go to row |
| `Enter` | Execute search |
| `Escape` | Close search/popup |
| `F3` / `⌘G` | Next match |
| `⇧F3` | Previous match |

## Contributing

Pull requests welcome! Please open an issue first to discuss major changes.

## License

MIT
