//! Integration test for the vxn2-app controller composition (ticket 0022).
//!
//! Exercises the round-trip through `vxn_core_app::Controller` against the
//! real `vxn2_engine::SharedParams` `ParamModel` / `Vxn2Params` impls.

use std::sync::Arc;

use vxn2_app::{
    Controller, MatrixRow, NoopPresetStore, ParamId, PresetMeta, UiEvent, ViewEvent, Vxn2Params,
    Vxn2UiCustom, Vxn2ViewCustom, tick_vxn2,
};
use vxn2_engine::SharedParams;
use vxn2_engine::params::id_of;

fn build_controller() -> (
    Controller<SharedParams>,
    std::sync::mpsc::Receiver<ViewEvent>,
    Arc<SharedParams>,
) {
    let model = Arc::new(SharedParams::new());
    let (ctrl, view_rx, _corpus) =
        Controller::new(model.clone(), Box::new(NoopPresetStore));
    (ctrl, view_rx, model)
}

/// Build a controller seeded the same way `vxn2-clap::new_main_thread`
/// does: "Init" as the synthetic preset name until the preset epic
/// ships.
fn build_controller_with_init() -> (
    Controller<SharedParams>,
    std::sync::mpsc::Receiver<ViewEvent>,
    Arc<SharedParams>,
) {
    let (mut ctrl, view_rx, model) = build_controller();
    ctrl.set_init_preset_meta(Some(PresetMeta {
        name: "Init".into(),
        ..Default::default()
    }));
    (ctrl, view_rx, model)
}

fn drain(rx: &std::sync::mpsc::Receiver<ViewEvent>) -> Vec<ViewEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

#[test]
fn set_param_round_trips_through_controller() {
    let (mut ctrl, view_rx, model) = build_controller();
    let vol_clap = id_of("master-volume").expect("master-volume present");
    let id = ParamId::new(vol_clap);

    let handle = ctrl.handle();
    handle.post(UiEvent::SetParam { id, plain: -3.0 }).unwrap();

    tick_vxn2(&mut ctrl);

    // Model side updated.
    assert!((model.get(vol_clap) - (-3.0)).abs() < 1e-5);

    // View side: one ParamChanged with matching plain / display.
    let events = drain(&view_rx);
    let mut saw = false;
    for ev in events {
        if let ViewEvent::ParamChanged {
            id: ev_id,
            plain,
            display,
            ..
        } = ev
        {
            if ev_id == id {
                assert!((plain - (-3.0)).abs() < 1e-5);
                assert_eq!(display, "-3.00 dB");
                saw = true;
            }
        }
    }
    assert!(saw, "no ParamChanged for master-volume on the view rx");
}

#[test]
fn matrix_row_custom_event_writes_model_no_echo() {
    let (mut ctrl, view_rx, model) = build_controller();

    let row = MatrixRow {
        source: 2,
        dest: 17,
        curve: 1,
        active: true,
        depth: 0.42,
    };
    let handle = ctrl.handle();
    handle
        .post(UiEvent::Custom(Box::new(Vxn2UiCustom::SetMatrixRow {
            slot: 3,
            row,
        })))
        .unwrap();

    tick_vxn2(&mut ctrl);

    // Model: row written through.
    let got = Vxn2Params::matrix_row(&*model, 3);
    assert_eq!(got, row);

    // Slot 3 is within the CLAP-automatable range (0..8), so depth also
    // landed in the CLAP table (mtx4-depth, 1-based mtx# = slot+1).
    let clap_id = id_of("mtx4-depth").expect("mtx4-depth present");
    assert!((model.get(clap_id) - 0.42).abs() < 1e-5);

    // Per ADR 0003 / E005 the SetMatrixRow handler no longer echoes:
    // the dirty bitset on SharedParams catches the drift and the CLAP
    // shell's pump pushes a fresh MatrixSnapshot. The controller test
    // doesn't run that pump, so no matrix view event lands here.
    let events = drain(&view_rx);
    let echoed = events.iter().any(|ev| match ev {
        ViewEvent::Custom(payload) => matches!(
            payload.downcast_ref::<Vxn2ViewCustom>(),
            Some(Vxn2ViewCustom::MatrixRowChanged { .. })
                | Some(Vxn2ViewCustom::MatrixSnapshot { .. })
        ),
        _ => false,
    });
    assert!(!echoed, "SetMatrixRow handler must not push view events");
}

#[test]
fn request_matrix_snapshot_emits_full_16_row_table() {
    let (mut ctrl, view_rx, model) = build_controller();

    // Seed a couple of distinct rows so the snapshot has something to
    // assert against.
    let row_a = MatrixRow {
        source: 1,
        dest: 4,
        curve: 0,
        active: true,
        depth: 0.1,
    };
    let row_b = MatrixRow {
        source: 7,
        dest: 12,
        curve: 2,
        active: false,
        depth: -0.25,
    };
    Vxn2Params::set_matrix_row(&*model, 2, row_a);
    Vxn2Params::set_matrix_row(&*model, 9, row_b);

    ctrl.handle()
        .post(UiEvent::Custom(Box::new(
            Vxn2UiCustom::RequestMatrixSnapshot,
        )))
        .unwrap();
    tick_vxn2(&mut ctrl);

    let mut got = None;
    for ev in drain(&view_rx) {
        if let ViewEvent::Custom(payload) = ev {
            if let Ok(custom) = payload.downcast::<Vxn2ViewCustom>() {
                if let Vxn2ViewCustom::MatrixSnapshot { rows } = *custom {
                    got = Some(rows);
                }
            }
        }
    }
    let rows = got.expect("MatrixSnapshot not emitted");
    assert_eq!(rows.len(), 16);
    assert_eq!(rows[2], row_a);
    assert_eq!(rows[9], row_b);
}

#[test]
fn matrix_row_slot_9_uses_extra_depth_storage() {
    let (mut ctrl, _view_rx, model) = build_controller();
    let row = MatrixRow {
        source: 5,
        dest: 21,
        curve: 0,
        active: true,
        depth: -0.6,
    };
    ctrl.handle()
        .post(UiEvent::Custom(Box::new(Vxn2UiCustom::SetMatrixRow {
            slot: 10, // past the 0..8 CLAP-automatable range
            row,
        })))
        .unwrap();
    tick_vxn2(&mut ctrl);

    let got = Vxn2Params::matrix_row(&*model, 10);
    assert_eq!(got, row);
    // No CLAP id is affected by a slot 9-16 write — spot-check against
    // mtx7-depth which the default patch leaves at descriptor default 0.0
    // (mtx5/mtx6 are now seeded for the default VoiceSpread → OpNPan
    // routings).
    let mtx7 = id_of("mtx7-depth").unwrap();
    assert_eq!(model.get(mtx7), 0.0);
}

/// Ticket 0029: when the page reports ready, the controller emits a
/// synthetic `PresetLoaded { name: "Init" }` BEFORE the full param
/// re-broadcast.
#[test]
fn editor_ready_emits_synthetic_init_preset_before_broadcast() {
    let (mut ctrl, view_rx, _model) = build_controller_with_init();
    ctrl.handle().post(UiEvent::EditorReady).unwrap();
    tick_vxn2(&mut ctrl);

    let events = drain(&view_rx);
    let preset_idx = events
        .iter()
        .position(|ev| matches!(ev, ViewEvent::PresetLoaded { .. }))
        .expect("synthetic PresetLoaded missing from EditorReady tick");
    let first_param_idx = events
        .iter()
        .position(|ev| matches!(ev, ViewEvent::ParamChanged { .. }))
        .expect("no ParamChanged events at all after EditorReady");
    assert!(
        preset_idx < first_param_idx,
        "synthetic PresetLoaded must precede the param re-broadcast"
    );
    match &events[preset_idx] {
        ViewEvent::PresetLoaded { meta, source, warnings } => {
            assert_eq!(meta.name, "Init");
            assert!(source.is_none());
            assert!(warnings.is_empty());
        }
        other => panic!("unexpected event at preset_idx: {other:?}"),
    }
}

/// Ticket 0029: prev/next with no corpus emits a Status the preset bar
/// surfaces as a toast.
#[test]
fn step_preset_with_empty_corpus_emits_status() {
    let (mut ctrl, view_rx, _model) = build_controller();
    ctrl.handle()
        .post(UiEvent::StepPreset { delta: 1 })
        .unwrap();
    tick_vxn2(&mut ctrl);
    let saw = drain(&view_rx).into_iter().any(|ev| {
        matches!(
            ev,
            ViewEvent::Status { line } if line == "No presets available"
        )
    });
    assert!(saw, "expected Status {{ line: \"No presets available\" }}");
}

/// Ticket 0029: Save / Save As both land on `UiEvent::SavePreset` →
/// `NoopPresetStore::user_save`, which returns the "not yet supported"
/// stub message wrapped into a Status by the shared controller.
#[test]
fn save_preset_through_noop_store_emits_not_yet_supported_status() {
    let (mut ctrl, view_rx, _model) = build_controller();
    ctrl.handle()
        .post(UiEvent::SavePreset {
            name: "Init".into(),
            folder: None,
        })
        .unwrap();
    tick_vxn2(&mut ctrl);
    let saw = drain(&view_rx).into_iter().any(|ev| match ev {
        ViewEvent::Status { line } => line.contains("Save not yet supported in this build"),
        _ => false,
    });
    assert!(
        saw,
        "expected Status containing 'Save not yet supported in this build'"
    );
}
