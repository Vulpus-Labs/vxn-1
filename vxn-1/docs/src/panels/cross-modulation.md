# Cross-modulation

VXN1 collapses what most synths spread across three distinct features — hard sync, FM/PM, and ring modulation — into one **Cross-Mod Type** selector with a single **Cross-Mod Amount** depth. The selected mode determines what Osc 2 does to Osc 1.

## Modes

| Type | What it does |
| --- | --- |
| **Off** | Independent oscillators. Bit-identical to the no-cross-mod fast path. |
| **Sync** | Hard sync. Osc 1 is the slave; Osc 2 is the master. Sub-sample phase reset on every Osc 2 wrap, with polyBLEP residual for band-limiting. Sweep Osc 1's pitch (typically via Pitch Env with Mod on) to get classic sync sweeps. |
| **FM** (PM) | Through-zero phase modulation. Osc 2's output offsets Osc 1's read phase. Labelled "FM" on the panel for familiarity — internally it is true phase modulation, which means pitch stays stable as the modulator's DC level moves. |
| **Ring** | Diode-bridge ring modulator (Parker DAFx-11 model). The ring output replaces Osc 1's mixer slot — Osc 1's normal waveform is gone, replaced by the ring product. |

## Amount

**Cross-Mod Amount** (0–4) maps to:

- **Sync**: ignored — sync is binary (mode selects whether sync happens; amount has no effect).
- **PM/FM**: phase deviation index. Higher values push Osc 1 into harsher inharmonic territory.
- **Ring**: ring depth (mixer level for the ring product).

A common starting point: PM at amount ~1.5 with Osc 2 a few semitones above Osc 1 gives a bell-like timbre.

## Cross-Mod Sweep

There is no dedicated cross-mod sweep route. The classic sound — a wide, envelope-driven pitch swing on the modulator only — is built from two parts on the [modulation panel](modulation.md):

- **Pitch Env Mod** switch (on the pitch route) — when on, the pitch envelope is routed *only* to the cross-mod modulator: Osc 1 under Sync, Osc 2 under PM / Ring / Off. Combine with the ±12 st **Pitch Env Dep** for the envelope swing.
- **Wheel→X-Mod** (on the mod-wheel routes) — ±48 st wide-pitch route from MIDI CC1, applies to both oscillators in parallel. Use it for hands-on sweeping.

Pair Pitch Env Mod + a high Pitch Env Dep with Cross-Mod Type = Sync, and you have the canonical sync-sweep lead. Same trick under PM gives a swept-index FM bell.

The Wheel→X-Mod route is *not* gated by Cross-Mod Type. With Off or Ring it acts as a normal pitch joystick (both oscillators move together); with Sync or PM it audibly drives the cross-mod timbre.

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
