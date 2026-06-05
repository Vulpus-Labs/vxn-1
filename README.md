# VXN

Monorepo for [Vulpus Labs](https://github.com/Vulpus-Labs) synthesizers.

| Subdir   | Status    | Notes                                                                        |
| -------- | --------- | ---------------------------------------------------------------------------- |
| `vxn-1/` | shipping  | 80s-style analogue polysynth (CLAP). See [vxn-1/README.md](vxn-1/README.md). |
| `vxn-2/` | in design | Operator-based architecture, fixed-point phase + approximated sines.         |

Each subdir is its own Cargo workspace with its own `Cargo.lock` and `xtask`.
Common code, as it emerges, will move into top-level `shared/` crates pulled by
path from both workspaces.

License: see [LICENSE.txt](LICENSE.txt) (MIT OR Apache-2.0).
