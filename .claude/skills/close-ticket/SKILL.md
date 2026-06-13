---
name: close-ticket
description: Close a vxn-2 ticket — verify acceptance criteria, append a Close-out section, git mv open→closed, and commit. Use when the user says "close ticket NNNN", "/close-ticket NNNN", or asks to mark a vxn-2 ticket done.
---

# Close a vxn-2 ticket

Ritual for closing a ticket in `vxn-2/tickets/`. Ticket id is a 4-digit string (e.g. `0068`). Take it from the user's request; if absent, ask which ticket.

## Steps

1. **Locate.** Find `vxn-2/tickets/open/NNNN-*.md`. If it's already in `closed/`, stop and say so. Read it.

2. **Verify acceptance criteria.** Read the `## Acceptance criteria` list. For each item, confirm it's genuinely satisfied in the current tree (code, tests, grep sweeps — whatever the criterion asserts). Do NOT just trust prior conversation. If a criterion is unmet, stop and report which — do not close a ticket whose work isn't done unless the user explicitly waives it.
   - Leave the `- [ ]` checkboxes **as-is** (unchecked). This repo does not tick them; the Close-out section is the record instead.

3. **Append Close-out section.** At the end of the ticket file add:

   ```markdown

   ## Close-out (YYYY-MM-DD)

   - <what shipped, per acceptance item — concrete>. File refs like
     [engine.rs:146](../../crates/vxn2-engine/src/engine.rs#L146), test names
     (`mod::tests::name`), grep-sweep results. One bullet per distinct change.
   ```

   Use today's date (check the environment's current date). Mirror the tone of existing Close-out sections: terse, file-and-test-referenced, states what was verified.

4. **Move.** `git mv vxn-2/tickets/open/NNNN-*.md vxn-2/tickets/closed/`.

5. **Commit.** Only if the user wants a commit (ask if unsure; they may be batching). Conventional Commits, scope `vxn-2`, ticket id in parens. Type matches the work: `feat`/`fix`/`refactor`/`docs`/`test`. Example:

   ```
   feat(vxn-2): RT-harden reset + sine table (0068)
   ```

   End the commit message with the Co-Authored-By trailer per the repo/global git convention. Don't push unless asked.

## Notes

- If the ticket belongs to an epic and is its last open child, mention that close-epic may now apply (see the close-epic skill).
- Acceptance verification is the point of this skill — the file move is mechanical. Spend the effort on step 2.
