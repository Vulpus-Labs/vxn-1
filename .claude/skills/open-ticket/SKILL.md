---
name: open-ticket
description: Scaffold a new worklist ticket — next id, frontmatter (incl. product), and the Summary/Acceptance-criteria/Notes skeleton. Use when the user says "open a ticket", "new ticket", "/open-ticket", or asks to file a ticket.
---

# Open a new ticket

Create a ticket in the unified worklist at repo root `tickets/open/`. The worklist spans both products — every ticket carries a `product:` field (`vxn-1` or `vxn-2`). Gather the topic from the user's request; ask for anything essential that's missing (at minimum a title, which product, and what the work is).

## Steps

1. **Next id.** Single global counter across the whole worklist. Scan `tickets/open/` (and `tickets/closed/` if it exists), take the highest 4-digit prefix, add 1, zero-pad to 4 digits:

   ```bash
   ls tickets/open tickets/closed 2>/dev/null | grep -oE '^[0-9]{4}' | sort -n | tail -1
   ```

   Ids are not per-product — the next number follows the global max regardless of product.

2. **Product.** Determine whether the work is `vxn-1` or `vxn-2` (ask if unclear). This sets the `product:` field and the relative paths to source.

3. **Slug.** Kebab-case the title, trimmed to something short. Filename = `NNNN-<slug>.md`.

4. **Epic.** If the work belongs to an open epic (`epics/open/EXXX-*.md` — match on the epic's own `product:`), set `epic:` to that id and link it from the Summary. Otherwise `epic: null`.

5. **Write the file** with this skeleton (match existing tickets' style — file-referenced, specific, DSP-vs-UX called out where relevant):

   ```markdown
   ---
   id: "NNNN"
   product: <vxn-1 or vxn-2>
   title: "<full descriptive title>"
   priority: medium
   created: YYYY-MM-DD
   epic: <EXXX or null>
   depends: []
   ---

   ## Summary

   <What's wrong / what's needed and why. Reference real code with
   [file.rs:line](../../<product>/crates/.../file.rs#L42) links. If it belongs to
   an epic, link it: Nth ticket of [EXXX](../../epics/open/EXXX-name.md).>

   ## Acceptance criteria

   - [ ] <concrete, checkable outcome — name the test or observable behaviour>
   - [ ] <...>

   ## Notes

   <Design pointers, related memories [[slug]], dependencies, what's out of scope.>
   ```

   Use today's date (check the environment's current date). Include a `## Design` section between Summary and Acceptance criteria when the approach isn't obvious from the Summary.

6. **Don't commit** unless asked — a freshly-opened ticket is usually staged alongside its first work commit.

## Notes

- `product:` is required — it determines source paths (`../../<product>/crates/...`) and the commit scope used when the ticket is closed.
- `depends:` lists ticket ids this one blocks on; leave `[]` if none.
- `priority:` is `low`/`medium`/`high` — default `medium` unless the user says otherwise.
