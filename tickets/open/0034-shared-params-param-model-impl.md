---
id: "0034"
title: SharedParams implements ParamModel; descriptor adaptor
priority: high
created: 2026-05-30
epic: E009
---

## Summary

Implement `vxn_app::ParamModel` for `vxn_engine::SharedParams` and
`vxn_app::ParamDescriptor` for `vxn_engine::ParamDesc`. After this
ticket, the controller can read/write parameter state through the
trait without knowing about the concrete engine types — the
precondition for VXN-2 reuse (ADR 0007 §2/§4).

## Acceptance criteria

- [ ] `impl ParamModel for SharedParams` in `vxn-engine`, delegating to
      the existing `get` / `set` / `get_normalized` / `set_normalized` /
      `gesture` / `set_gesture` methods.
- [ ] `impl ParamDescriptor for ParamDesc`, delegating to the existing
      `label` / `min` / `max` / `default` / `to_fader` / `from_fader` /
      `display`.
- [ ] `ParamModel::descriptor` returns the descriptor for a given
      `ParamId`, sourcing from the existing `desc_for_clap_id` lookup.
- [ ] Trait-object usage: `Arc<dyn ParamModel> = Arc::new(SharedParams::new())`
      compiles and runs end-to-end against a smoke test that writes a
      value through the trait and reads it back through the concrete
      type. (Confirms no orphan-rules surprises and no `Send`/`Sync`
      regressions on the audio path.)
- [ ] `cargo test --workspace` passes.

## Notes

`SharedParams` is `Send + Sync` (atomics under the hood); the trait's
`Send + Sync` bound (ADR 0007 §4) costs nothing here. The audio thread
keeps using the concrete type — the trait is for the controller's
generic surface only.

Descriptor lookup: the current API returns `Option<&'static ParamDesc>`
(static table). Keep the static lifetime in the impl; the trait method
can return `Option<&dyn ParamDescriptor>` and the impl narrows that to
`&'static dyn ParamDescriptor` via coercion.

No controller changes yet — 0035 plugs the trait into `Controller<M>`.
