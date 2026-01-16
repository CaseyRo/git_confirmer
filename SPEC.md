# Git Confirmer Spec (OpenSpec-style)

## Problem
I want a fast way to see the state of many local git repositories under one or more root folders and commit all changes with a consistent message, while still allowing per-repo notes and the ability to skip repos.

## Goals
- Discover git repositories under configured root folders.
- Show a compact status summary for each repo in a TUI.
- Allow selecting which repos to commit and skipping the rest.
- Generate a base commit message and optionally append a per-repo comment.
- Commit all changes for selected repos.

## Non-Goals
- Managing remotes, rebases, merges, or advanced workflows.
- Authentication or network operations (fetch/push).
- Handling submodules specially.

## Users
- Developers with many local repos who want batch commits.

## UX Overview
- TUI list with repo name, path, branch, and change counts.
- Keyboard controls for navigation, selection, and committing.
- Per-repo optional comment edit.

## Inputs
- Root folders to scan (default: ~/dev).
- Base commit message (default: "all changes in files").

## Outputs
- TUI display of repo statuses.
- Commits made to selected repos.

## Data Model
- Repo item: { path, name, branch, ahead, behind, staged, unstaged, untracked, selected, comment }

## Key Flows
1. Scan roots and build repo list.
2. Show list in TUI.
3. User selects repos (default selection: repos with changes).
4. User optionally edits per-repo comment.
5. Commit selected repos using: "<base message> - <comment>" when comment present.

## Error Handling
- If a repo has no changes, skip commit and report.
- If git commands fail, show error in status area.

## Constraints
- No network access required.
- Must run from a shell script entry point.

## Testing
- Manual run against known repos.

## Open Questions
- Should default selection include clean repos? (current: no)
- Should commit message include repo name? (current: no)
