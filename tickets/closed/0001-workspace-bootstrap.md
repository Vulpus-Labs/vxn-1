---
id: "0001"
title: Root workspace bootstrap + skeleton crates
priority: high
created: 2026-06-06
epic: E001
---

## Summary

Promote the repo root to a Cargo workspace. Add empty
`crates/vxn-core-utils`, `vxn-core-app`, `vxn-core-ui-web`,
`vxn-core-clap` skeleton crates so 0002–0005 can land in parallel
without `Cargo.toml` conflicts. vxn-1 and vxn-2 stay building
unchanged.

## Acceptance criteria

- [ ] `/Cargo.toml` at repo root declares a workspace with members
      `crates/vxn-core-utils`, `crates/vxn-core-app`,
      `crates/vxn-core-ui-web`, `crates/vxn-core-clap`, plus
      `vxn-1/Cargo.toml` and `vxn-2/Cargo.toml` either as path
      members or default-members exclusion (decide in this ticket).
- [ ] `[workspace.package]` pins `edition = "2024"`,
      `rust-version = "1.85"`, `license = "MIT OR Apache-2.0"`,
      `authors = ["Vulpus Labs"]`, `version = "0.0.0"`, matching
      vxn-2's current pin.
- [ ] `[workspace.dependencies]` carries the clack git rev currently
      pinned in vxn-2/Cargo.toml — single source of truth across all
      shared crates.
- [ ] Each `vxn-core-*` crate has a `Cargo.toml` with package metadata
      inheriting from workspace, an empty `src/lib.rs` (no `pub`
      items, just `//! crate doc` line), and a unit test that
      asserts `2 + 2 == 4` (proves test infra wires up).
- [ ] `cargo check --workspace`, `cargo test --workspace`,
      `cargo build --workspace --release` all succeed.
- [ ] vxn-1 and vxn-2 individual builds (their own `cargo build`
      from `vxn-1/` and `vxn-2/` directories) still succeed
      unchanged — confirm no inherited `[profile.release]` regression
      caused by the new root workspace.
- [ ] Single `/Cargo.lock` at repo root. `vxn-1/Cargo.lock` and
      `vxn-2/Cargo.lock` either deleted (if folded in) or left as
      sub-workspace locks (if vxn-1/vxn-2 stay as sub-workspaces).
      Decision recorded in commit message.

## Notes

Two viable layouts:

**A. Single flat workspace.** vxn-1/* and vxn-2/* crates become
direct members of the root workspace. One `Cargo.lock`, one target
dir, one `cargo test --workspace` runs everything. Disruptive: every
vxn-1 + vxn-2 path-dep needs re-pointing relative to the new root.

**B. Workspace-of-workspaces.** Root workspace owns only
`crates/vxn-core-*`. vxn-1 and vxn-2 remain their own workspaces and
consume root crates via `path = "../crates/vxn-core-app"` etc. Three
`Cargo.lock`s, smaller move, but `clack` git rev pinned in three
places risks drift.

Recommend A for the cleanest end state. B is the rollback if A blows
up the existing CI in non-obvious ways.

Skeleton crates exist only so 0002–0005 can edit a real
`crates/vxn-core-utils/src/` etc. without each needing to first
create the crate. Empty `lib.rs` is fine; do NOT pre-declare modules
that don't yet exist or `cargo check` fails.
