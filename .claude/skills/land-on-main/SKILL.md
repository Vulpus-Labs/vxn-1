---
name: land-on-main
description: Commit and push work directly to main — this repo's workflow has no PRs and no feature branches, everything lands on main. Use when the user says "commit and push", "land this", "push to main", or "/land-on-main".
---

# Land work on main

This repo (`Vulpus-Labs/vxn-1`, remote `origin`) ships straight to `main`. **No PRs, no feature branches** — commit on `main` and push. This is the deliberate workflow, so the usual "branch before committing" reflex does **not** apply here.

## Steps

1. **Confirm the user wants to push.** Pushing to `main` is outward-facing and hard to undo. Only proceed when the user has asked to commit/push or land. If they only asked to commit, stop after step 4.

2. **Review what's staged vs unstaged.** `git status` + `git diff`/`git diff --cached`. Stage deliberately — don't blanket `git add -A` if unrelated changes are in the tree. Confirm you're on `main` (`git branch --show-current`); if not, ask before switching.

3. **Sanity-check the change builds/tests** if it touches code, when practical (`cargo test --workspace` / `vitest`, or at least `cargo check`). Don't push red. If you skip checks, say so.

4. **Commit.** Conventional Commits, scope `vxn-2` for vxn-2 work. Subject ≤ ~70 chars, imperative; reference ticket/epic id in parens when applicable. Body only when the "why" isn't obvious from the subject. End with the trailer:

   ```
   Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
   ```

   Examples from history:
   ```
   feat(vxn-2): RT-harden reset + sine table (0068)
   fix(vxn-2): multiplicative level mod + DX7 feedback scale — kill crackle (0076-0079)
   docs(vxn-2): close tickets 0075-0079 (level-mod crackle remediation)
   ```

5. **Pull then push.** `main` tracks `origin/main`. Pull first to avoid a rejected push:

   ```bash
   git pull --rebase origin main && git push origin main
   ```

   If the rebase hits conflicts, stop and surface them — don't force-anything. Never `git push --force` to `main`.

## Notes

- Closing a ticket/epic? The close-ticket / close-epic skills produce the commit; this skill is the push half. They compose.
- If multiple logical changes are mixed in the tree, prefer separate commits over one blob — match the granularity of recent history.
