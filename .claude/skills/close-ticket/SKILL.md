---
name: close-ticket
description: Close a worklist ticket — verify acceptance criteria, append a Close-out section, git mv open→closed, and commit. Use when the user says "close ticket NNNN", "/close-ticket NNNN", or asks to mark a ticket done.
---

# Close a ticket

Ritual for closing a ticket in the unified worklist at repo root `tickets/`. Tickets span both products (`vxn-1` and `vxn-2`); the `product:` frontmatter field says which. Ticket id is a 4-digit string (e.g. `0027`), globally unique across the worklist. Take it from the user's request; if absent, ask which ticket.

## Steps

1. **Locate.** Find `tickets/open/NNNN-*.md`. If it's already in `tickets/closed/`, stop and say so. Read it — note its `product:` field, you'll need it for source paths and the commit scope.

2. **Verify acceptance criteria.** Read the `## Acceptance criteria` list. For each item, confirm it's genuinely satisfied in the current tree (code, tests, grep sweeps — whatever the criterion asserts). Search under the ticket's product subtree (`vxn-1/` or `vxn-2/`). Do NOT just trust prior conversation. If a criterion is unmet, stop and report which — do not close a ticket whose work isn't done unless the user explicitly waives it.
   - Leave the `- [ ]` checkboxes **as-is** (unchecked). This repo does not tick them; the Close-out section is the record instead.

3. **Append Close-out section.** At the end of the ticket file add:

   ```markdown

   ## Close-out (YYYY-MM-DD)

   - <what shipped, per acceptance item — concrete>. File refs like
     [engine.rs:146](../../vxn-2/crates/vxn2-engine/src/engine.rs#L146) — the path
     is `../../<product>/crates/...`, product per the ticket's `product:` field.
     Test names (`mod::tests::name`), grep-sweep results. One bullet per distinct change.
   ```

   Use today's date (check the environment's current date). Mirror the tone of existing Close-out sections: terse, file-and-test-referenced, states what was verified.

4. **Move.** `git mv tickets/open/NNNN-*.md tickets/closed/` (creates `tickets/closed/` if it doesn't exist yet).

5. **Commit.** Only if the user wants a commit (ask if unsure; they may be batching). Conventional Commits, scope = the ticket's `product` (`vxn-1` or `vxn-2`), ticket id in parens. Type matches the work: `feat`/`fix`/`refactor`/`docs`/`test`. Example:

   ```
   feat(vxn-2): RT-harden reset + sine table (0027)
   ```

   End the commit message with the Co-Authored-By trailer per the repo/global git convention. Don't push unless asked.

## Notes

- If the ticket belongs to an epic and is its last open child, mention that close-epic may now apply (see the close-epic skill).
- Acceptance verification is the point of this skill — the file move is mechanical. Spend the effort on step 2.
