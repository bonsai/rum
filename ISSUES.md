# Issue Tracker — rum

## Current Issues (Delegated to Jules)

| # | GitHub Issue | Title | Status | Jules Order File |
|---|--------------|-------|--------|------------------|
| 1 | #1 | feat: 3ペイン + BOX + たて24行の紙 + 裏写り | ✅ Done (20815c8) | rum-007-3pane-layout.md |
| 2 | #2 | ISSUE-001: Chira Command Dependency | 🟡 Partial (0ef53b0) | rum-001-chira-dependency.md |
| 3 | #3 | ISSUE-002: SQLite Concurrency Risks | 🟡 In Progress (41a981f) | rum-002-sqlite-concurrency.md |
| 4 | #4 | ISSUE-005: Command Execution Error Handling Gap | 🟡 Partial (0ef53b0) | rum-005-command-errors.md |
| 5 | #5 | ISSUE-004: Missing Front Matter on New Text Files | ✅ Fixed (0ccbb4d) | rum-004-front-matter.md |
| 6 | #6 | ISSUE-003: Configuration Path Expansion Edge Cases | 🔴 Open | rum-003-path-expansion.md |
| 7 | #7 | ISSUE-006: Undo/Redo State Persistence Across Modes | 🔴 Open | rum-006-undo-redo.md |

## Local Fixes Applied (on `issue-creator` branch)

| Commit | Issue | Description |
|--------|-------|-------------|
| 20815c8 | #1 | 3-pane layout — Sidebar (25%) + Editor (50%) + Notes/Memos (25%) |
| 41a981f | #3 | SQLite concurrency — transactions + exponential backoff retry |
| 0ccbb4d | #5 | new_note ensures front matter for all files |
| 0ef53b0 | #2, #4 | chira command error handling — clearer "not found" message |

## Jules Sessions Launched

All 7 issues have Jules sessions running (launched via `mcp__jules_cli__jules_launch`):

- rum-001-chira-dependency.md → issue 1
- rum-002-sqlite-concurrency.md → issue 2
- rum-003-path-expansion.md → issue 3
- rum-004-front-matter.md → issue 4
- rum-005-command-errors.md → issue 5
- rum-006-undo-redo.md → issue 6
- rum-007-3pane-layout.md → issue 7

## Dependency Graph

```
#3 SQLite Concurrency ──→ #4 Command Errors ──→ #1 3-Pane Layout
#6 Path Expansion ──→ #5 Front Matter ──→ #1 3-Pane Layout
#2 Chira Dependency ──→ #4 Command Errors
#7 Undo/Redo ──→ #1 3-Pane Layout
```

**Critical Path**: #3 → #6 → #5 → #4 → #1

## Next Steps

1. Monitor Jules session progress via `gh run list --branch issue-creator`
2. Review Jules patches when complete
3. Apply and test patches locally
4. Merge to main when all issues resolved
