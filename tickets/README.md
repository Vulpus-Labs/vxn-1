# tickets

`open/` and `closed/` hold one markdown file per ticket, `NNNN-slug.md`, at the
repo root (`tickets/`). Epics live alongside in `epics/open/` and
`epics/closed/`, named `ENNN-slug.md`.

**Numbering convention:** vxn-1 and vxn-2 tickets now share a *single* unified
counter in this `tickets/` directory, so each ticket number is globally unique.
Which synth a ticket belongs to is recorded in the `product:` frontmatter field
(`vxn-1` or `vxn-2`), not by its number or directory. A bare ticket number
refers unambiguously to the one file `tickets/{open,closed}/NNNN-*.md`. New
tickets take the next free number in the shared sequence.
