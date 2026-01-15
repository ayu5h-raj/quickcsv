# Agent Instructions for QuickCSV

This file contains instructions for AI coding agents working on this project.

## Project Overview

QuickCSV is a high-performance CSV viewer for macOS built with Rust and egui.

## Tech Stack

- **Language**: Rust
- **GUI**: eframe/egui
- **File I/O**: memmap2 (memory-mapped files)
- **CSV Parsing**: csv crate
- **Threading**: std::thread with parking_lot locks

## Key Architecture Decisions

1. **Memory Mapping**: We use `memmap2` for zero-copy file access. Never load entire files into RAM.
2. **Progressive Loading**: Show data immediately, index in background.
3. **Virtualized Rendering**: Only parse/render visible rows.
4. **Row Cache**: Cache last 2000 parsed rows for smooth scrolling.

## Coding Standards

- Run `cargo fmt` before committing
- Run `cargo clippy -- -D warnings` to check for warnings
- Use inline format args: `format!("{value}")` not `format!("{}", value)`

## Build Profiles

The project has two release profiles optimized for different use cases:

### Development: `release-fast` profile
```bash
cargo run --profile release-fast
cargo build --profile release-fast
```

**Use this for:**
- Local development and testing
- Quick iteration when working on features
- Performance testing during development

**Features:**
- Fast compile times (~50-60 seconds)
- High optimization level (opt-level = 3)
- Parallel compilation (codegen-units = 16)
- No LTO for faster linking

### Production: `release` profile
```bash
cargo build --release
```

**Use this for:**
- Final releases and distribution
- GitHub Actions CI/CD builds
- Creating optimized binaries for users

**Features:**
- Slower compile times (~2-5 minutes)
- Maximum optimization with Link-Time Optimization (LTO)
- Single codegen unit for best cross-crate optimization
- Smallest and fastest binary output

**Note:** The `release` profile is automatically used by GitHub Actions for all releases. For local development, prefer `release-fast` for faster iteration.

### Web/WASM Builds

The project supports web builds using Trunk. Build settings are configured in `Trunk.toml`.

**Development (fast iteration):**
```bash
trunk serve
```
- Uses `release-fast` profile for balanced speed (~30-60 seconds initial, ~10-20 seconds incremental)
- High optimization (opt-level 3) but no LTO for faster compilation
- Auto-reloads on file changes
- Opens browser at http://127.0.0.1:8080

**Production (optimized):**
```bash
trunk build --release
```
- Uses full release profile with LTO for maximum optimization
- Slower build (~2-5 minutes) but smallest WASM output
- Output in `dist/` directory

## Git Workflow

1. Create feature branch: `feature/feature-name` or `fix/bug-name`
2. Make changes and commit with conventional commits:
   - `feat:` for new features
   - `fix:` for bug fixes
   - `perf:` for performance improvements
   - `docs:` for documentation
   - `ci:` for CI/CD changes
3. Push branch and create PR
4. After merge, create tag for release: `git tag v0.X.X && git push origin v0.X.X`

## Release Process

1. Update version in `Cargo.toml` (both `package.version` and `package.metadata.bundle.version`)
2. Commit and push to main
3. Create and push tag: `git tag vX.X.X && git push origin vX.X.X`
4. GitHub Actions will automatically:
   - Build the macOS app bundle
   - Create GitHub Release
   - Update Homebrew tap

### Troubleshooting Release Issues

- **If tag already exists**: Delete and recreate it:
  ```bash
  git tag -d vX.X.X
  git push origin --delete vX.X.X
  git tag vX.X.X
  git push origin vX.X.X
  ```

- **YAML heredoc issues**: Avoid using heredocs (`<<`) in GitHub Actions for multi-line content that contains special characters (like Ruby's `#{}` interpolation). Use `printf` statements instead.

## Homebrew Tap

- Tap repo: `ayu5h-raj/homebrew-tap`
- Auto-updated on release via GitHub Actions
- Uses `postflight` to remove quarantine attribute (app is unsigned)

## Files Overview

- `src/main.rs` - All application code
- `Cargo.toml` - Dependencies and app metadata
- `ARCHITECTURE.md` - Technical documentation for developers
- `.github/workflows/` - CI/CD pipelines
  - `ci.yml` - Runs on every push (format check, clippy, build)
  - `release.yml` - Runs on version tags (build, release, homebrew update)

## Performance Considerations

- For large fields (>100KB), truncate display to 200 chars
- Skip search highlighting on fields >100KB
- Use `Cow<str>` to avoid unnecessary allocations
- Cache parsed rows to avoid repeated parsing

## Common Tasks

### Adding a new feature
1. Create branch
2. Implement in `src/main.rs`
3. Test with large CSV files
4. Run `cargo fmt && cargo clippy -- -D warnings`
5. Commit, push, create PR

### Making a release
1. Bump version in Cargo.toml
2. Merge to main
3. Tag and push: `git tag vX.X.X && git push origin vX.X.X`
4. Monitor workflow at: https://github.com/ayu5h-raj/quickcsv/actions
