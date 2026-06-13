---
name: open-ticket
description: Scaffold a new vxn-2 ticket — next id, frontmatter, and the Summary/Acceptance-criteria/Notes skeleton. Use when the user says "open a ticket", "new ticket", "/open-ticket", or asks to file a vxn-2 ticket.
---

# Open a new vxn-2 ticket

Create a ticket in `vxn-2/tickets/open/`. Gather the topic from the user's request; ask for anything essential that's missing (at minimum a title and what the work is).

## Steps

1. **Next id.** Scan both `vxn-2/tickets/open/` and `vxn-2/tickets/closed/` filenames, take the highest 4-digit prefix, add 1, zero-pad to 4 digits:

   ```bash
   ls vxn-2/tickets/open vxn-2/tickets/closed | grep -oE '^[0-9]{4}' | sort -n | tail -1
   ```

2. **Slug.** Kebab-case the title, trimmed to something short. Filename = `NNNN-<slug>.md`.

3. **Epic.** If the work belongs to an open epic (`vxn-2/epics/open/EXXX-*.md`), set `epic:` to that id and link it from the Summary. Otherwise `epic: null`.

4. **Write the file** with this skeleton (match existing tickets' style — file-referenced, specific, DSP-vs-UX called out where relevant):

   ```markdown
   ---
   id: "NNNN"
   title: "<full descriptive title>"
   priority: medium
   created: YYYY-MM-DD
   epic: <EXXX or null>
   depends: []
   ---

   ## Summary

   <What's wrong / what's needed and why. Reference real code with
   [file.rs:line](../../crates/.../file.rs#L42) links. If it belongs to an
   epic, link it: Nth ticket of [EXXX](../../epics/open/EXXX-name.md).>

   ## Acceptance criteria

   - [ ] <concrete, checkable outcome — name the test or observable behaviour>
   - [ ] <...>

   ## Notes

   <Design pointers, related memories [[slug]], dependencies, what's out of scope.>
   ```

   Use today's date (check the environment's current date). Include a `## Design` section between Summary and Acceptance criteria when the approach isn't obvious from the Summary.

5. **Don't commit** unless asked — a freshly-opened ticket is usually staged alongside its first work commit.

## Notes

- `depends:` lists ticket ids this one blocks on; leave `[]` if none.
- `priority:` is `low`/`medium`/`high` — default `medium` unless the user says otherwise.
