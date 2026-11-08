# Issue Tracker

## Current Issues

### [ISSUE-001] Chira Command Dependency
**Problem**: The `:md` and `:sync` commands rely on the external `chira` CLI tool being installed and accessible in PATH. If missing, users get generic "Failed to run chira" errors without guidance.
**Steps to Reproduce**:
1. Ensure `chira` is not installed or not in PATH
2. Run `rum path/to/notes --md`
3. Observe error: `Failed to run chira: Os { code: 2, kind: NotFound }`
**Suggested Fix**: 
- Add pre-check for `chira` existence
- Provide clear installation instructions in help text
- Consider bundling minimal chira binary or providing alternative import methods

### [ISSUE-002] SQLite Concurrency Risks
**Problem**: Multiple rapid operations on SQLite DB (e.g., quick save/load cycles) may cause database locking without proper transaction handling.
**Evidence**: 
- `save_note()` uses simple `Connection::open()` without wrapping in transaction
- No retry logic for `Database is locked` errors
**Suggested Fix**: 
- Wrap DB operations in `rusqlite::Transaction` for atomicity
- Implement exponential backoff for lock contention
- Add DB connection pooling for frequent operations

### [ISSUE-003] Configuration Path Expansion Edge Cases
**Problem**: Tilde (`~`) expansion fails in certain contexts (e.g., Windows paths with backslashes, mixed separators).
**Evidence**: 
- `expand("~/dir\\file")` may not resolve correctly on Windows
- `expand("~\\")` creates invalid paths
**Suggested Fix**: 
- Improve `expand()` to handle Windows-specific path quirks
- Add validation for expanded paths before use
- Consider using `directories` crate for more robust path resolution

## Additional Issues Discovered During Code Review

### [ISSUE-004] Missing Front Matter on New Text Files
**Problem**: When creating new `.txt` files (non-markdown), front matter is not automatically generated, but the code path might not handle this consistently across all edit scenarios.
**Evidence**: 
- `new_note()` calls `ensure_front_matter()`, which only works for markdown files per `ensure_front_matter()` logic
- `.txt` files saved without front matter may lose structural metadata when re-opened
**Suggested Fix**: 
- Ensure all new files get appropriate metadata initialization
- Consider automatic front matter generation for all file types or explicit user opt-in

### [ISSUE-005] Command Execution Error Handling Gap
**Problem**: The `execute_command` method has inconsistent error reporting - some commands update `status_message` while others silently fail or produce terminal output only.
**Evidence**: 
- `:w` (save) updates status on success/failure
- `:q` shows message only if modified, otherwise sets `should_quit`
- `:db` and `:dir` show usage message if arg missing, but don't prevent mode switch
- Some error paths don't restore normal mode properly
**Suggested Fix**: 
- Standardize error handling pattern across all commands
- Ensure mode always returns to Normal on error or completion
- Add validation before mode transitions

### [ISSUE-006] Undo/Redo State Persistence Across Modes
**Problem**: The undo/redo stack doesn't properly persist or reset when switching between Normal, Insert, and Visual modes, potentially causing unexpected state.
**Evidence**: 
- `TextBuffer::undo()` and `redo()` operate on `history` and `redo_stack`
- Mode changes don't trigger stack clearing or preservation
- User might expect undo history to be mode-specific or global
**Suggested Fix**: 
- Clarify undo/redo semantics in documentation
- Add mode-aware history management if needed
- Consider user expectations for cross-mode state

## Proposed Next Steps
1. Address Issue-001 by enhancing command error messages
2. Implement transaction safety in SQLite operations (Issue-002)
3. Refactor path expansion logic (Issue-003)
4. Fix front matter handling for text files (Issue-004)
5. Standardize command error handling (Issue-005)
6. Review undo/redo cross-mode behavior (Issue-006)

Let's start with Issue-001 - the most user-visible problem.