# QuickCSV

A blazing-fast CSV viewer for macOS that handles files from 100MB to 2GB+ with zero lag.

![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)
![Platform](https://img.shields.io/badge/platform-macOS-blue.svg)
![License](https://img.shields.io/badge/license-MIT-green.svg)

## Features

- 🚀 **Instant File Opening** - Memory-mapped I/O means no waiting
- 📊 **Handle Massive Files** - 100MB to 2GB+ with smooth scrolling
- ⚡ **Virtualized Table** - Only visible rows are rendered
- 🔍 **Fast Search** - Search millions of rows without freezing
- 🎨 **Native macOS Look** - Dark theme, native file dialogs
- 📁 **Drag & Drop** - Just drop your CSV file

## Download

Download the latest release from the [Releases page](../../releases).

Or build from source:

```bash
# Clone the repo
git clone https://github.com/YOUR_USERNAME/quickcsv.git
cd quickcsv

# Build release version
cargo build --release

# Run
cargo run --release
```

## Create macOS App Bundle

```bash
# Install cargo-bundle (one time)
cargo install cargo-bundle

# Create .app bundle
cargo bundle --release

# The app will be at: target/release/bundle/osx/QuickCSV.app
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

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `⌘O` | Open file |
| `⌘F` | Find/Search |
| `Enter` | Execute search |
| `Escape` | Close search |
| `F3` | Next match |
| `⇧F3` | Previous match |
| `⌘Q` | Quit |

## Distribution

### For Users
Download `QuickCSV.app` from Releases and drag to your Applications folder.

### For Developers

**Create .app bundle:**
```bash
cargo bundle --release
# Output: target/release/bundle/osx/QuickCSV.app
```

**Create ZIP for distribution:**
```bash
cd target/release/bundle/osx
zip -r QuickCSV-macos.zip QuickCSV.app
```

**Create DMG (optional):**
```bash
brew install create-dmg
create-dmg \
  --volname "QuickCSV" \
  --window-size 600 400 \
  --icon "QuickCSV.app" 150 185 \
  --app-drop-link 450 185 \
  QuickCSV.dmg \
  target/release/bundle/osx/QuickCSV.app
```

### GitHub Releases
1. Tag a release: `git tag v0.1.0 && git push --tags`
2. Build the app bundle: `cargo bundle --release`
3. Create ZIP: `zip -r QuickCSV-v0.1.0-macos.zip target/release/bundle/osx/QuickCSV.app`
4. Upload to GitHub Releases

## License

MIT

## Contributing

Pull requests welcome! Please open an issue first to discuss major changes.