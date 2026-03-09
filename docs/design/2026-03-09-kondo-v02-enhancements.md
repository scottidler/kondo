# Design Document: Kondo v0.2 Enhancements

**Author:** Scott Idler
**Date:** 2026-03-09
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

Five enhancements to kondo's file organization engine: fix dry-run output wording, handle duplicate files via content hashing, add exclude/override patterns to config, introduce hash-based short-circuiting for unchanged directories, and decide on relative path preservation behavior.

## Problem Statement

### Background

Kondo v0.1.0 scans configurable source directories (e.g., `~/Downloads`, `~/Desktop`, or any other directory) and moves files to destination directories (`~/Pictures`, `~/Music`, `~/Videos`, `~/Documents`, etc.) based on extension rules in `kondo.yml`. It optionally dashifies filenames and manages its own cron schedule. It works, but has rough edges that matter for real daily use.

### Problem

1. **Dry-run says "would move"** — verbose and inconsistent with the actual "moved" output. Should just say "move" in both modes.
2. **No duplicate handling** — when a file already exists at the destination, kondo skips it silently. The source copy stays in Downloads forever. If the files are identical, the source should be cleaned up. If different, the user needs to know.
3. **No exceptions** — can't exclude specific files or patterns. A `.json` config file you're actively editing in Downloads gets moved to Documents whether you want it or not.
4. **No short-circuiting** — kondo re-scans every file on every run even if nothing changed. When running via cron every 15 minutes, this is wasteful.
5. **Relative path preservation** — currently flattens: `~/Downloads/project/notes.pdf` becomes `~/Documents/notes.pdf`. Needs a conscious decision.

### Goals
- Fix dry-run output text (trivial)
- Deduplicate identical files at source when destination already has a copy
- Add `exclude` patterns to config for files/directories to skip
- Cache directory state to skip unchanged sources
- Document relative path behavior and add config option if needed

### Non-Goals
- Recursive scanning of source subdirectories (future feature)
- File deletion or trash integration
- Conflict resolution UI (interactive prompts)
- Watching for filesystem events (inotify/fswatch) — cron is the mechanism

## Proposed Solution

### Phase 1: Dry-run output fix + after-action report

Change `"would move"` to `"move"` everywhere. Both dry-run and real mode use the same verb. Dry-run is already distinguished by the yellow `(dry run - no files will be moved)` banner.

Replace the simple summary line with a structured after-action report printed at the end of every run. The report has two sections: a summary with counts, and details for anything that wasn't a clean move.

**Report structure:**

```
kondo report (dry run)
  move       42 file(s)
  dedup       3 file(s)
  skip       12 file(s)
  conflict    1 file(s)
  exclude     5 file(s)
  error       0 file(s)

details:
  dedup     photo.png (identical to ~/Pictures/photo.png, source removed)
  skip      random.zip (no matching rule)
  skip      image.png (already exists at ~/Pictures/image.png)
  conflict  report.pdf (differs from ~/Documents/report.pdf)
  exclude   temp.crdownload (matched *.crdownload)
  error     broken.png (permission denied)
```

Rules:
- Summary section always prints, even when all counts are zero
- Details section only prints lines for non-move outcomes (moves are the happy path, no need to re-list them unless `--verbose`)
- With `--verbose`, details also lists every move
- Categories with zero entries are omitted from details
- In dry-run mode, header says `kondo report (dry run)` and verbs are present tense (`move`, `dedup`, `skip`)
- In normal mode, header says `kondo report` and verbs are past tense (`moved`, `deduped`, `skipped`)
- Each detail line is colored: move/dedup green, skip yellow, conflict/error red, exclude dim

**Implementation:**

New `Report` struct to accumulate results:
```rust
pub struct Report {
    pub entries: Vec<ReportEntry>,
}

pub struct ReportEntry {
    pub action: Action,
    pub source: PathBuf,
    pub destination: Option<PathBuf>,
    pub reason: Option<String>,
}

pub enum Action {
    Move,
    Dedup,
    Skip,
    Conflict,
    Exclude,
    Error,
}
```

- `organize()` returns `Report` instead of `(usize, usize)`
- `Report::print(dry_run: bool, verbose: bool)` handles all formatting
- Each code path (move, skip, dedup, conflict, exclude, error) pushes a `ReportEntry` instead of printing inline

**Files:** `src/report.rs` (new module), `src/main.rs` (integrate report)

### Phase 2: Duplicate handling with content hashing

When `dest.exists()`, instead of just skipping, compare file contents:

1. Compare file sizes first (fast reject)
2. If sizes match, compute SHA-256 of both files
3. If hashes match: remove the source file (it's a duplicate), log it
4. If hashes differ: warn the user, skip the move, count as a conflict
5. In dry-run mode: report what would happen but don't delete anything
6. Skip symlinks — only process regular files (add explicit `!path.is_symlink()` check)

**New dependency:** `sha2` crate for SHA-256.

**Config addition:**
```yaml
# What to do when destination file already exists with identical content
# options: skip, dedup (remove source duplicate)
on-duplicate: dedup
```

Serde config: `#[serde(rename = "on-duplicate")]` on the field, with `DuplicateAction` deriving `Serialize`/`Deserialize` and using `#[serde(rename_all = "lowercase")]`.

**New output categories:**
```
  dedup  ~/Downloads/photo.png (identical to ~/Pictures/photo.png, source removed)
  conflict  ~/Downloads/report.pdf (differs from ~/Documents/report.pdf, skipped)
```

**Summary line update:**
```
moved 12 file(s), 3 deduped, 1 conflict, 5 skipped
```

**Edge case — dashify collision:** Two source files (e.g., `My Photo.png` and `my_photo.png`) may dashify to the same name `my-photo.png`. The second file should be skipped with a warning rather than silently overwriting.

**Files:** `src/main.rs` (move_file, organize, summary), `src/config.rs` (new field + enum), `Cargo.toml` (sha2 dep)

### Phase 3: Exclude patterns

Add an `exclude` list to config — glob patterns matched against the filename (not the full path). Uses the `glob` crate's pattern matching.

**Config addition:**
```yaml
exclude:
  - "*.part"        # partial downloads
  - "*.crdownload"  # chrome partial downloads
  - ".~lock.*"      # libreoffice lock files
  - "*.tmp"
```

**Implementation:**
- New `exclude` field on `Config` as `Vec<String>` (default: empty)
- In `organize()`, before extension lookup, check if filename matches any exclude pattern
- Use `glob::Pattern` for matching (standard glob syntax)
- Excluded files don't count as skipped — they're invisible

**New dependency:** `glob` crate (for `Pattern::matches`).

**Files:** `src/config.rs` (new field), `src/main.rs` (filter step in organize), `Cargo.toml` (glob dep)

### Phase 4: Hash-based short-circuiting

Cache a snapshot of each source directory so unchanged directories are skipped entirely.

**Cache file:** `~/.cache/kondo/state.json`

**Cache structure:**
```json
{
  "config_hash": "abc123...",
  "dirs": {
    "/home/user/Downloads": {
      "entries": {
        "photo.png": { "size": 12345, "mtime": 1709913600 },
        "report.pdf": { "size": 67890, "mtime": 1709913500 },
        "random.zip": { "size": 99999, "mtime": 1709913400 }
      },
      "scanned_at": 1709913700
    }
  }
}
```

Note: ALL files in the directory are cached, not just ones matching extension rules. This ensures new files appearing (even with unmatched extensions) are detected as a directory change.

**Algorithm:**
1. Compute SHA-256 of the config file content. If it differs from cached `config_hash`, invalidate entire cache.
2. On scan, collect `(filename, size, mtime)` for ALL files in the source directory
3. Compare against cached state for that directory
4. If identical: skip the directory entirely, log "no changes"
5. If different: process normally, then update cache
6. CLI flag `--no-cache` to force full scan
7. Write cache atomically: write to temp file then rename, preventing corruption from interrupted runs

**New dependency:** `serde_json` (for cache file — simpler than YAML for machine-written data).

**Files:** `src/cache.rs` (new module), `src/main.rs` (integrate cache check), `src/cli.rs` (--no-cache flag), `Cargo.toml` (serde_json dep)

### Phase 5: Relative path preservation

Add `--preserve-paths` as both a CLI flag and a config option. Default: `false` (flatten).

**Config addition:**
```yaml
# preserve subdirectory structure when moving files
# ~/Downloads/project/notes.pdf -> ~/Documents/project/notes.pdf (true)
# ~/Downloads/project/notes.pdf -> ~/Documents/notes.pdf (false)
preserve-paths: false
```

Serde config: `#[serde(rename = "preserve-paths")]` on the field.

**CLI flag:**
```
--preserve-paths    Preserve subdirectory structure when moving files
```

CLI flag overrides config value when set.

**Implementation:**
- When `preserve_paths` is true and scanning recursively, compute the relative path from the source root to the file, then append that relative path to the destination directory
- Example: source `~/Downloads`, file `~/Downloads/project/notes.pdf`, dest `~/Documents` → `~/Documents/project/notes.pdf`
- When false (default): flatten as today → `~/Documents/notes.pdf`
- This requires recursive directory scanning, which is currently not implemented — kondo only processes top-level files. Phase 5 adds recursive scanning gated behind this flag.
- When `preserve-paths: false` (default), behavior is unchanged from v0.1 — only top-level files are processed, subdirectories are ignored.

**Files:** `src/config.rs` (new field), `src/cli.rs` (new flag), `src/main.rs` (recursive scan + path logic)

### Data Model

Updated `Config` struct:
```rust
pub struct Config {
    pub dashify: bool,
    pub sources: Vec<String>,
    pub rules: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub exclude: Vec<String>,                    // Phase 3
    #[serde(rename = "on-duplicate", default)]
    pub on_duplicate: DuplicateAction,           // Phase 2
    #[serde(rename = "preserve-paths", default)]
    pub preserve_paths: bool,                    // Phase 5
}

#[derive(Default)]
#[serde(rename_all = "lowercase")]
pub enum DuplicateAction {
    #[default]
    Skip,  // current behavior
    Dedup, // remove source if identical
}
```

Cache struct (new file):
```rust
pub struct DirSnapshot {
    pub entries: HashMap<String, FileEntry>,
    pub scanned_at: u64,
}

pub struct FileEntry {
    pub size: u64,
    pub mtime: u64,
}

pub struct Cache {
    pub dirs: HashMap<String, DirSnapshot>,
    pub config_hash: String,
}
```

### Implementation Plan

| Phase | Description | New deps | New files |
|-------|-------------|----------|-----------|
| 1 | Dry-run output fix + after-action report | none | `src/report.rs` |
| 2 | Dedup with content hashing | `sha2` | none |
| 3 | Exclude patterns | `glob` | none |
| 4 | Hash-based short-circuit cache | `serde_json` | `src/cache.rs` |
| 5 | Preserve-paths option + recursive scan | none | none |

Each phase is independently shippable and testable.

## Alternatives Considered

### Alternative 1: Use file metadata only (no content hashing) for dedup
- **Description:** Trust size + mtime match as "identical"
- **Pros:** Faster, no new dependency
- **Cons:** Not reliable — same size doesn't mean same content, mtime can be modified
- **Why not chosen:** SHA-256 is fast enough for the file sizes involved (images, documents), and correctness matters when deleting source files

### Alternative 2: Regex instead of glob for exclude patterns
- **Description:** Use regex patterns for excludes
- **Pros:** More powerful matching
- **Cons:** Overkill for filename matching, harder for users to write correctly
- **Why not chosen:** Glob patterns are the natural fit for filename matching and what users expect

### Alternative 3: SQLite for cache instead of JSON
- **Description:** Store directory state in a SQLite database
- **Pros:** Atomic writes, queryable
- **Cons:** Heavy dependency for a simple key-value cache
- **Why not chosen:** JSON file is sufficient — the cache is small (one entry per file in source dirs) and only written at end of scan

### Alternative 4: Always preserve relative paths
- **Description:** `~/Downloads/subdir/file.pdf` → `~/Documents/subdir/file.pdf` with no option to flatten
- **Pros:** Preserves user's directory structure
- **Cons:** Creates fragmented directory trees in destinations, defeats the purpose for most users
- **Why not chosen:** Made it an opt-in config/CLI option (`--preserve-paths`) instead. Default is flatten.

## Technical Considerations

### Dependencies

| Crate | Phase | Purpose | Size impact |
|-------|-------|---------|-------------|
| `sha2` | 2 | Content hashing for dedup | Small (pure Rust) |
| `glob` | 3 | Filename pattern matching | Tiny |
| `serde_json` | 4 | Cache file serialization | Already transitively present |

### Performance

- **Phase 2 (dedup):** SHA-256 only computed when sizes match, which is the minority case. Typical file sizes (images, PDFs) hash in microseconds.
- **Phase 4 (cache):** Collecting `stat()` metadata for directory entries is an order of magnitude faster than any I/O. A cache hit skips all file processing entirely. For cron runs on unchanged directories, this reduces runtime from ~100ms to ~1ms.

### Security

- **Phase 2:** File deletion (dedup) is the only destructive operation kondo performs. It requires content hash verification before removing source files. The `on_duplicate: dedup` config must be explicitly set.
- **Phase 4:** Cache file at `~/.cache/kondo/state.json` contains only filenames, sizes, and timestamps — no sensitive content.

### Testing Strategy

- **Phase 1:** Visual inspection of dry-run output
- **Phase 2:** Create test files with identical/different content, verify dedup removes only identical sources
- **Phase 3:** Create test files matching exclude patterns, verify they're ignored
- **Phase 4:** Run twice on unchanged directory, verify second run short-circuits via log output

### Rollout Plan

Each phase is a separate commit. Bump to v0.2.0 after all phases land.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Dedup deletes a source file user still needs | Low | High | Only dedup when content hash matches; `on_duplicate: skip` is the safe default |
| Cache goes stale (file modified without mtime change) | Low | Low | `--no-cache` flag; cache uses mtime+size, not just mtime |
| Glob pattern matches too broadly | Medium | Medium | Document patterns clearly in config comments; dry-run shows what's excluded |
| SHA-256 slow on very large files | Low | Low | Size comparison first rejects most non-matches; large media files (GB+) rarely duplicate |
| Recursive scan with preserve-paths moves unexpected files | Medium | Medium | Off by default; dry-run first; only processes files matching extension rules |
| Dashify collision (two files → same name) | Low | Medium | Detect at move time, skip second file with warning |

## Open Questions
- [x] Should relative paths be preserved? **Opt-in via `--preserve-paths` config/CLI option. Default: flatten.**
- [ ] Should `on-duplicate` default to `skip` or `dedup`? Leaning `skip` for safety.
- [ ] Should the cache include a TTL or max-age? Probably not — mtime comparison is sufficient.
- [ ] Should excludes support full path globs (e.g., `~/Downloads/temp/**`) or only filename patterns?

## References
- Kondo v0.1.0 source: `~/repos/scottidler/kondo/`
- dashify: `~/.cargo/bin/dashify` (filename normalization)
- SHA-256 crate: `sha2` on crates.io
