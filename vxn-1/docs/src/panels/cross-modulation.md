# Cross-modulation

VXN1 collapses what most synths spread across three distinct features — hard sync, FM/PM, and ring modulation — into one **Cross-Mod Type** selector with a single **Cross-Mod Amount** depth. The selected mode determines what Osc 2 does to Osc 1.

## Modes

| Type | What it does |
| --- | --- |
| **Off** | Independent oscillators. Bit-identical to the no-cross-mod fast path. |
| **Sync** | Hard sync. Osc 1 is the slave (carrier); Osc 2 is the master. Sub-sample phase reset on every Osc 2 wrap, with polyBLEP residual for band-limiting. Detune Osc 2 up to get classic sync sweeps. |
| **FM** (PM) | Through-zero phase modulation. Osc 2's output offsets Osc 1's read phase. Labelled "FM" on the panel for familiarity — internally it is true phase modulation, which means pitch stays stable as the modulator's DC level moves. |
| **Ring** | Diode-bridge ring modulator (Parker DAFx-11 model). The ring output replaces Osc 1's mixer slot — Osc 1's normal waveform is gone, replaced by the ring product. |

## Amount

**Cross-Mod Amount** (0–4) maps to:

- **Sync**: ignored — sync is binary (mode selects whether sync happens; amount has no effect).
- **PM/FM**: phase deviation index. Higher values push Osc 1 into harsher inharmonic territory.
- **Ring**: ring depth (mixer level for the ring product).

A common starting point: PM at amount ~1.5 with Osc 2 a few semitones above Osc 1 gives a bell-like timbre.

## Cross-Mod Sweep

The **Cross-Mod Sweep** modulation route (on the [modulation panel](modulation.md#cross-mod-sweep)) wide-sweeps Osc 2's pitch by ±48 st, driven by Env 1, Env 2, or the Mod Wheel. This is the classic "filter sweep but on the modulator" sound — dramatic with sync, useful for sci-fi PM textures.

The sweep is **mode-gated**: it only takes effect when Cross-Mod Type ≠ Off. With Off, the sweep depth knobs are inert.

## Aliasing notes

Cross-mod can fold significant high-frequency energy back into the audible band:

- **Sync**: polyBLEP-band-limited; safe at 1× oversample for most material.
- **PM with non-sine modulator**: aliasing is *by design* — many vintage PM sounds rely on it. The 4× / 8× oversampling settings are the escape hatch if a specific patch needs cleaning up.
- **Ring**: cleaner than PM, but ring of two saws will alias under 2×. Push oversampling for bright ring patches.

See [Master](master.md) for the global oversampling setting.

## Parameters

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| Cross-Mod Type | Off / Sync / FM / Ring | Off | enum | Mode selector |
| Cross-Mod Amount | 0–4 | 0 | depth | Index for PM, mixer level for Ring; ignored by Sync |
