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
- **ALWAYS create separate branches for features and fixes** - never commit directly to main
- Add comprehensive tests for new features and bug fixes

## egui-Specific Guidelines

### Context Menus and Sense Usage

When implementing right-click context menus in egui:

- **IMPORTANT**: Use `Sense::hover()` for widgets that need context menus, NOT `Sense::click()`
- `Sense::click()` only captures primary (left) mouse button clicks
- `Sense::hover()` enables both left-click interactions AND right-click context menus
- Example:
  ```rust
  let response = ui.add(Label::new(text).sense(Sense::hover()));
  response.context_menu(|ui| {
      // Context menu items here
  });
  ```

### Testing UI Features

- Test new UI features with both `--profile release-fast` AND `--release` builds
- Some issues only appear in optimized release builds
- Always verify context menus, drag-and-drop, and mouse interactions work in release builds

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

**CRITICAL**: Always create a separate branch for every feature or fix. Never commit directly to main.

1. Create feature branch: `feature/feature-name` or `fix/bug-name`
   ```bash
   git checkout -b feature/my-feature
   # or
   git checkout -b fix/my-bugfix
   ```
2. Make changes and commit with conventional commits:
   - `feat:` for new features
   - `fix:` for bug fixes
   - `perf:` for performance improvements
   - `docs:` for documentation
   - `chore:` for version bumps, dependency updates
   - `ci:` for CI/CD changes
3. Push branch and create PR
   ```bash
   git push -u origin feature/my-feature
   ```
4. After merge, bump version and create tag for release (see Release Process below)

## Release Process

### Version Bumping

Follow semantic versioning (MAJOR.MINOR.PATCH):
- **PATCH** (0.10.1 → 0.10.2): Bug fixes, small improvements
- **MINOR** (0.10.2 → 0.11.0): New features, non-breaking changes
- **MAJOR** (0.11.0 → 1.0.0): Breaking changes, major redesigns

When bumping version:
1. Update BOTH version fields in `Cargo.toml`:
   - `package.version` (line 3)
   - `package.metadata.bundle.version` (line 25)
2. Commit on a branch (e.g., `chore/bump-version-0.10.2`)
3. Push and create PR

### Creating a Release

1. After version bump PR is merged to main
2. Create and push tag:
   ```bash
   git checkout main
   git pull
   git tag v0.10.2
   git push origin v0.10.2
   ```
3. GitHub Actions will automatically:
   - Build the macOS app bundle with `cargo build --release`
   - Create GitHub Release
   - Update Homebrew tap
   - Deploy web version to GitHub Pages

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
1. Create feature branch: `git checkout -b feature/feature-name`
2. Implement in `src/main.rs` (or appropriate module)
3. Add tests in the same file using `#[cfg(test)]` module
4. Test with large CSV files
5. Test with BOTH profiles:
   - `cargo run --profile release-fast` (quick testing)
   - `cargo build --release && ./target/release/quickcsv` (final verification)
6. Run `cargo fmt && cargo clippy -- -D warnings`
7. Run tests: `cargo test --lib`
8. Commit, push branch, and create PR

### Fixing a bug
1. Create fix branch: `git checkout -b fix/bug-name`
2. Fix the bug in appropriate file(s)
3. Add test case that reproduces and validates the fix
4. Test with BOTH `--profile release-fast` AND `--release` builds
5. Run `cargo fmt && cargo clippy -- -D warnings`
6. Run tests: `cargo test --lib`
7. Commit, push branch, and create PR

### Making a release
1. Create version bump branch: `git checkout -b chore/bump-version-X.X.X`
2. Bump version in Cargo.toml (both fields)
3. Push branch and create PR
4. After merge, create and push tag: `git tag vX.X.X && git push origin vX.X.X`
5. Monitor workflow at: https://github.com/ayu5h-raj/quickcsv/actions
