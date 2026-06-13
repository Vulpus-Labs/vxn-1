---
name: close-epic
description: Close a vxn-2 epic — confirm all child tickets are closed, set status closed, git mv open→closed, and commit. Use when the user says "close epic EXXX", "/close-epic", or asks to finish a vxn-2 epic.
---

# Close a vxn-2 epic

Ritual for closing an epic in `vxn-2/epics/`. Epic id is `EXXX` (e.g. `E007`). Take it from the user's request; if absent, ask.

## Steps

1. **Locate.** Find `vxn-2/epics/open/EXXX-*.md`. If already in `closed/`, stop and say so. Read it.

2. **Confirm all child tickets closed.** Identify the epic's tickets (those whose frontmatter `epic:` equals this id, and any enumerated in the epic's Scope). Check none remain in `vxn-2/tickets/open/`:

   ```bash
   grep -l 'epic: EXXX' vxn-2/tickets/open/*.md
   ```

   If any are still open, stop and list them — do not close the epic until its tickets are closed (unless the user explicitly waives a ticket, e.g. deferred/won't-do, in which case note that).

3. **Verify the epic's own acceptance/scope.** Epics carry their own acceptance bullets under Goal/Scope. Confirm the headline outcomes hold in the tree, same spirit as close-ticket step 2.

4. **Set status.** Edit frontmatter `status: open` → `status: closed`.

5. **Move.** `git mv vxn-2/epics/open/EXXX-*.md vxn-2/epics/closed/`.

6. **Commit** (if the user wants one). Conventional Commits, scope `vxn-2`, epic id in parens:

   ```
   feat(vxn-2): close EXXX — <one-line epic outcome>
   ```

   End with the Co-Authored-By trailer per repo/global git convention. Don't push unless asked.

## Notes

- Epics, like tickets, don't tick their acceptance checkboxes — `status: closed` plus the move is the close signal.
- If closing a ticket just emptied this epic, that's the usual trigger to run this skill.
