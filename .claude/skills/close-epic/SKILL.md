---
name: close-epic
description: Close a worklist epic — confirm all child tickets are closed, set status closed, git mv open→closed, and commit. Use when the user says "close epic EXXX", "/close-epic", or asks to finish an epic.
---

# Close an epic

Ritual for closing an epic in the unified worklist at repo root `epics/`. Epics span both products (`vxn-1` and `vxn-2`); the `product:` frontmatter field says which. Epic id is `EXXX` (e.g. `E010`), globally unique across the worklist. Take it from the user's request; if absent, ask.

## Steps

1. **Locate.** Find `epics/open/EXXX-*.md`. If already in `epics/closed/`, stop and say so. Read it — note its `product:` field for the commit scope.

2. **Confirm all child tickets closed.** Identify the epic's tickets (those whose frontmatter `epic:` equals this id, and any enumerated in the epic's Scope). Check none remain in `tickets/open/`:

   ```bash
   grep -l 'epic: EXXX' tickets/open/*.md
   ```

   If any are still open, stop and list them — do not close the epic until its tickets are closed (unless the user explicitly waives a ticket, e.g. deferred/won't-do, in which case note that).

3. **Verify the epic's own acceptance/scope.** Epics carry their own acceptance bullets under Goal/Scope. Confirm the headline outcomes hold in the tree (under the epic's product subtree), same spirit as close-ticket step 2.

4. **Set status.** Edit frontmatter `status: open` → `status: closed`.

5. **Move.** `git mv epics/open/EXXX-*.md epics/closed/` (creates `epics/closed/` if it doesn't exist yet).

6. **Commit** (if the user wants one). Conventional Commits, scope = the epic's `product` (`vxn-1` or `vxn-2`), epic id in parens:

   ```
   feat(vxn-2): close E013 — Windows parity shipped
   ```

   End with the Co-Authored-By trailer per repo/global git convention. Don't push unless asked.

## Notes

- Epics, like tickets, don't tick their acceptance checkboxes — `status: closed` plus the move is the close signal.
- If closing a ticket just emptied this epic, that's the usual trigger to run this skill.
