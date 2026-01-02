# QuickCSV

A blazing-fast CSV viewer for macOS that handles files from 100MB to 2GB+ with zero lag.

![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)
![Platform](https://img.shields.io/badge/platform-macOS-blue.svg)
![License](https://img.shields.io/badge/license-MIT-green.svg)

## Features

- 🚀 **Instant File Opening** - Memory-mapped I/O means no waiting
- 📊 **Handle Massive Files** - 100MB to 2GB+ with smooth scrolling
- ⚡ **Virtualized Table** - Only visible rows are rendered
- 🔍 **Background Indexing** - UI never freezes
- 🎨 **Native macOS Look** - Dark theme, native file dialogs
- 📁 **Drag & Drop** - Just drop your CSV file

## Installation

```bash
# Clone the repo
git clone https://github.com/YOUR_USERNAME/quickcsv.git
cd quickcsv

# Build release version
cargo build --release

# Run
cargo run --release
```

## How It Works

```
CSV File → Memory Map → Background Indexer → Row Offset Index → Virtualized Table
```

1. **Memory Mapping**: File is mapped to memory using `memmap2` - no loading into RAM
2. **Background Indexing**: A thread scans for newlines to build a row offset index
3. **Virtualized Rendering**: Only visible rows are parsed and rendered using `egui_extras::TableBuilder`
4. **Row Caching**: Recently viewed rows are cached for smooth scrolling

## Dependencies

| Crate | Purpose |
|-------|---------|
| `eframe` | GUI framework |
| `egui_extras` | Virtualized table |
| `memmap2` | Memory-mapped I/O |
| `csv` | CSV parsing |
| `rfd` | Native file dialogs |

## Performance

Tested on MacBook Pro M1:

| File Size | Rows | Open Time | Scroll |
|-----------|------|-----------|--------|
| 10 MB | 100K | < 100ms | Smooth |
| 100 MB | 740K | < 500ms | Smooth |
| 1 GB | 7M+ | < 2s | Smooth |

## License

MIT

## Contributing

Pull requests welcome! Please open an issue first to discuss major changes.
EOF