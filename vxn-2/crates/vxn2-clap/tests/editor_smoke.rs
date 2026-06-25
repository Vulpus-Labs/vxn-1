//! Editor smoke (ticket 0032).
//!
//! Closes the IPC plumbing loop: simulates a JS-side `set_param` message
//! flowing through the IPC parser, into the controller's `UiEvent`
//! queue, through `tick_vxn2`, and into `SharedParams`. Asserts the
//! audio thread (or any other reader of `SharedParams`) sees the new
//! value. Also covers `begin_gesture` / `end_gesture` brackets — the
//! shape the page emits around a fader drag or a numeric-entry popup
//! commit (ticket 0030).
//!
//! Scope intentionally bounded to "JS → IPC → controller → ParamModel"
//! per the ticket's "ONE path" note. The WebView itself is NOT
//! instantiated here — wry's `WebViewBuilder::new_as_child` requires a
//! live NSView and a running AppKit event loop, which a `cargo test`
//! context can't safely provide without flakiness. The mounting code
//! is covered by the existing host-level integration in `smoke.rs` and
//! manual host playback (the ticket's "open in Bitwig" acceptance).
//!
//! macOS-gated to match the ticket. The simulated path itself is
//! platform-independent, but the WebView build dep graph compiles
//! conditionally on Linux/Windows in CI environments without the
//! GTK / WebView2 prerequisites; gating keeps this test stable.

#![cfg(target_os = "macos")]

use std::sync::Arc;

use vxn2_app::{NoopPresetStore, tick_vxn2};
use vxn2_engine::Vxn2PresetStore;
use vxn2_engine::params::id_of;
use vxn2_engine::shared::SharedParams;
use vxn_core_app::Controller;
use vxn_core_ui_web::parse_ui_event;

fn simulate_ipc(controller: &mut Controller<SharedParams>, body: &str) {
    let parse_custom = vxn2_ui_web::parse_custom_ui_for_test();
    let ev = parse_ui_event(body, Some(&parse_custom))
        .unwrap_or_else(|| panic!("IPC body did not parse: {body}"));
    controller
        .handle()
        .post(ev)
        .expect("controller channel rejected event");
}

fn build_controller() -> (Controller<SharedParams>, Arc<SharedParams>) {
    let shared = Arc::new(SharedParams::new());
    let (controller, _view_rx, _corpus) =
        Controller::new(shared.clone(), Box::new(NoopPresetStore));
    (controller, shared)
}

/// JS dispatches `{op:"set_param", id, plain}`. After one `tick_vxn2`
/// the shared store reflects the new value — the path the audio
/// thread reads from on its next block.
#[test]
fn set_param_round_trips_into_shared_params() {
    let (mut controller, shared) = build_controller();
    let decay = id_of("reverb-decay").unwrap();
    let body = format!(r#"{{"op":"set_param","id":{decay},"plain":7.5}}"#);
    simulate_ipc(&mut controller, &body);
    tick_vxn2(&mut controller);
    assert!(
        (shared.get(decay) - 7.5).abs() < 1e-5,
        "shared.get({decay}) = {} (want 7.5)",
        shared.get(decay)
    );
}

/// Numeric-entry popup commit (ticket 0030) brackets a `set_param` with
/// begin/end gesture events. After tick the value lands AND the
/// gesture flag clears — the diff pump (0031) wouldn't be suppressed.
#[test]
fn gesture_bracketed_set_param_lands_and_clears_gesture() {
    let (mut controller, shared) = build_controller();
    let algo = id_of("algo").unwrap();
    simulate_ipc(
        &mut controller,
        &format!(r#"{{"op":"begin_gesture","id":{algo}}}"#),
    );
    simulate_ipc(
        &mut controller,
        &format!(r#"{{"op":"set_param","id":{algo},"plain":12.0}}"#),
    );
    simulate_ipc(
        &mut controller,
        &format!(r#"{{"op":"end_gesture","id":{algo}}}"#),
    );
    tick_vxn2(&mut controller);
    assert!(
        (shared.get(algo) - 12.0).abs() < 1e-3,
        "algo {} (want 12.0)",
        shared.get(algo)
    );
    assert!(
        !shared.gesture(algo),
        "gesture flag still set after end_gesture",
    );
}

/// Normalised-write opcode: `{op:"set_param_norm", id, norm}` lands at
/// the descriptor-resolved plain value. Sanity-checks the taper
/// inversion in `ParamModel::set_normalized`.
#[test]
fn set_param_norm_round_trips_via_descriptor() {
    let (mut controller, shared) = build_controller();
    let vol = id_of("master-volume").unwrap();
    let body = format!(r#"{{"op":"set_param_norm","id":{vol},"norm":1.0}}"#);
    simulate_ipc(&mut controller, &body);
    tick_vxn2(&mut controller);
    let plain = shared.get(vol);
    let desc = vxn2_engine::desc_for_clap_id(vol).unwrap();
    assert!(
        (plain - desc.max).abs() < 1e-3,
        "norm=1.0 maps to {plain} (want descriptor max {})",
        desc.max
    );
}

/// VXN2-specific opcode: `set_matrix_row` rides `UiEvent::Custom` and
/// the vxn2 controller's tick handler applies it to the per-patch
/// matrix table on `SharedParams`.
#[test]
fn custom_set_matrix_row_routes_through_controller() {
    let (mut controller, shared) = build_controller();
    let body = r#"{
        "op": "set_matrix_row",
        "slot": 3,
        "row": {"source": 5, "dest": 17, "curve": 1, "active": true, "depth": 0.5}
    }"#;
    simulate_ipc(&mut controller, body);
    tick_vxn2(&mut controller);
    let row = shared.matrix_row_raw(3);
    assert_eq!(row.source, 5);
    assert_eq!(row.dest, 17);
    assert!(row.active);
    assert!((row.depth - 0.5).abs() < 1e-5, "depth {} (want 0.5)", row.depth);
}

/// Full preset-load path: JS dispatches `{op:"load_factory", index}`; the
/// shared backend parses it, the controller loads the factory blob through
/// `Vxn2PresetStore` and restores it into the model. After one tick the
/// store reflects the factory preset's params. We target `Brass/Analog Brass`
/// — algo 1, distinct from the default patch's algo 5 — a clean witness that
/// the preset landed. Located by name rather than a fixed index so adding
/// presets in alpha-earlier categories (e.g. Bass) doesn't break the test.
#[test]
fn load_factory_round_trips_into_shared_params() {
    let shared = Arc::new(SharedParams::new());
    let (mut controller, _view_rx, corpus) =
        Controller::new(shared.clone(), Box::new(Vxn2PresetStore::new()));
    // The published corpus carries the embedded factory bank. Find Analog
    // Brass by name; `load_factory` indexes this same sorted listing.
    let brass_index = {
        let c = corpus.lock().unwrap();
        assert!(c.factory.len() >= 5, "factory corpus too small: {}", c.factory.len());
        let idx = c
            .factory
            .iter()
            .position(|m| m.name == "Analog Brass")
            .expect("factory bank should contain Analog Brass");
        assert_eq!(c.factory[idx].category.as_deref(), Some("Brass"));
        idx
    };

    let algo = id_of("algo").unwrap();
    assert_eq!(shared.get(algo), 5.0, "default patch should be algo 5");

    simulate_ipc(
        &mut controller,
        &format!(r#"{{"op":"load_factory","index":{brass_index}}}"#),
    );
    tick_vxn2(&mut controller);

    assert_eq!(
        shared.get(algo),
        1.0,
        "Analog Brass (factory 0) should load algo 1, got {}",
        shared.get(algo)
    );
    // feedback 3.0 is part of that preset; the default patch is feedback 6.0.
    let fb = id_of("feedback").unwrap();
    assert!(
        (shared.get(fb) - 3.0).abs() < 1e-4,
        "feedback {} (want 3.0)",
        shared.get(fb)
    );
}

/// Unknown opcode is silently dropped — host/page version skew can't
/// crash the controller.
#[test]
fn unknown_opcode_drops_silently() {
    let parse_custom = vxn2_ui_web::parse_custom_ui_for_test();
    let ev = parse_ui_event(r#"{"op":"hypothetical_future_thing"}"#, Some(&parse_custom));
    assert!(ev.is_none(), "future opcode should not parse: {ev:?}");
}
