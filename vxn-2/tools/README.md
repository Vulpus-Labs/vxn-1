# vxn-2 tools

Repo-side scripts that are run by hand, not part of the build.

## DX7 factory-bank converter (E026 / ticket 0126)

`dx7_to_vxn2.py` translates the Yamaha DX7 factory ROM voices into vxn-2 factory
preset TOMLs under `crates/vxn2-engine/presets/factory/<Category>/`.
`dx7decode.py` is the `.syx` voice unpacker it depends on.

### Inputs (not committed)

The eight DX7 factory cartridge dumps — `rom1a.syx … rom4b.syx` — are **Yamaha
ROM data and are deliberately not committed** to this repo. Supply your own
dumps and point the converter at them via, in priority order:

1. `$VXN2_DX7_ROMS` — a directory holding the eight `.syx` files;
2. `tools/roms/` — drop them here (gitignored);
3. `/tmp` — the legacy scratch location.

### Regenerate the bank

```sh
cd vxn-2/tools
VXN2_DX7_ROMS=/path/to/roms python3 dx7_to_vxn2.py
# include_dir! emits no rerun-if-changed, so force the embed to refresh:
touch ../crates/vxn2-engine/src/factory.rs
cargo test -p vxn2-engine --lib factory   # validates the whole bank parses
```

The converter `clean()`s every non-`KEEP` preset and rewrites it, so the output
is deterministic. The 15 `KEEP` presets (5 hand-made + the original non-DX7
factory voices, incl. Mark II E-Piano) are never touched.

### Master-volume gain-match (the 0126 re-sweep)

Under the DX7 **log** level curve (ADR 0007) the old carrier-*count* heuristic
left the bank too quiet. `master_volume()` now estimates each patch's peak
loudness from the **log amplitude of its carriers' output levels**
(`Σ 2^((OL-99)/8)` over the algorithm's carriers) and gain-matches to
`TARGET_PEAK_DB` (clamped to `[-24, +6]` dBFS).

This is a calibrated **starting point** — it ignores FM brightness, EG sustain,
and feedback, so bright/sustained patches read quieter than they sound and
percussive ones louder. The final pass is by ear (or a measured-RMS render):
adjust `TARGET_PEAK_DB`, or hand-edit individual `master-volume` lines, and
re-run. The 5 hand-made `KEEP` presets are gain-matched by hand, not by this
heuristic.
