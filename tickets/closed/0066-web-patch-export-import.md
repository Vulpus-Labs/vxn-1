---
id: "0066"
product: vxn-2
title: "Patch export/import — download/upload file + URL share link"
priority: medium
created: 2026-06-15
epic: E019
depends: ["0065"]
---

## Summary

Fifth and final ticket of
[E019](../../epics/open/E019-web-persistence-presets-state.md). Let a user share
a patch off-device: export the current patch to a downloadable file and/or an
encode-in-URL share link, and import it back. Builds on the snapshot byte
channel
([`snapshot_bytes` / `restore_from_bytes`](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L118))
and the corpus storage from 0063/0064.

## Design

- **File export/import.** Download the current patch as a `.toml` (the existing
  name-keyed format, [[vxn1-preset-system]]) via a Blob/anchor; import via a
  file picker that parses it through the same `user_load` path and applies it (or
  offers to save it into the corpus).
- **URL share link.** Encode the snapshot compactly (base64url of the blob, or
  the TOML gzipped) into a URL fragment (`#patch=…`, kept out of the query so it
  isn't sent to any server). On load, if a `#patch=` fragment is present, decode
  and apply it before `EditorReady`. Cap the size; if a patch is too big for a
  practical URL, fall back to file-only and surface that.
- Reuse desktop format so an exported web patch imports on desktop and vice
  versa (no format divergence — epic acceptance).

## Acceptance criteria

- [ ] A patch can be exported to a file and re-imported, reproducing the params.
- [ ] A share-link URL round-trips: open it in a fresh tab and the patch
      applies.
- [ ] An exported file imports on the desktop build (format parity).
- [ ] Malformed file/URL input is rejected gracefully (no crash, user-visible
      message).

## Notes

- Share-link decode runs before `EditorReady` so the seed broadcast carries the
  imported values, same ordering as 0065's restore.
- Depends on 0065 (the full-state codec) and 0063/0064 (corpus + storage).
- Closing this ticket closes E019 — verify the epic's acceptance list end to end.
