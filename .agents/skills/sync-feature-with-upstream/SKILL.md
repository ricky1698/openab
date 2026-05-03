---
name: sync-feature-with-upstream
description: Keep a feature branch rebased on the latest upstream/main while syncing the fork's origin/main safely. Use when asked to sync a branch with upstream/main, refresh a branch against upstream, or repeat the fork-main sync plus feature-branch rebase workflow.
---

# Sync Feature With Upstream

Use this project-level skill to keep an OpenAB feature branch current with `upstream/main` and keep fork `origin/main` fast-forwarded to the same commit.

## Safety Rules

- Never use `git reset --hard`, `git checkout -- <path>`, or other destructive commands unless the user explicitly requests them.
- Never force push with `--force`; use `--force-with-lease` only after confirming the feature branch was rebased and the user expects the remote branch to be updated.
- Do not overwrite unrelated working tree changes. If the worktree is dirty, stop and ask whether to stash, commit, or leave changes alone.
- Only sync `origin/main` when it is a strict fast-forward or already equal to `upstream/main`.
- If `origin/main` has fork-only commits not in `upstream/main`, stop and ask the user how to proceed.

## Workflow

1. Fetch all refs:
   `git fetch --all --prune`

2. Check current state:
   `git status --short --branch`
   `git branch --show-current`

3. Confirm `origin/main` can be safely synced:
   `git merge-base --is-ancestor origin/main upstream/main`

4. If the command exits `0`, sync fork main:
   `git push origin upstream/main:main`

5. Verify main refs match:
   `git rev-parse --short origin/main`
   `git rev-parse --short upstream/main`
   `git log --oneline --left-right --cherry-pick origin/main...upstream/main`

6. Rebase the current feature branch onto latest upstream:
   `git rebase upstream/main`

7. If conflicts occur, resolve minimally, then continue:
   `git rebase --continue`

8. Verify the resulting branch:
   `git status --short --branch`
   `git log --oneline --decorate -8`
   `git diff --stat upstream/main...HEAD`

9. Run OpenAB verification when time permits or before pushing:
   `cargo fmt`
   `cargo clippy -- -D warnings`
   `cargo test`

10. If the feature branch already has a remote and the user wants it updated, push safely:
    `git push --force-with-lease origin <branch>`

## Reporting

Report these facts concisely:

- Whether `origin/main` was already synced or was updated.
- The final `origin/main` and `upstream/main` short hashes.
- The rebased feature branch name and top commit.
- Whether verification commands were run and their result.
