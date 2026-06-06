//! Integration test for the vxn2-app controller composition (ticket 0022).
//!
//! Exercises the round-trip through `vxn_core_app::Controller` against the
//! real `vxn2_engine::SharedParams` `ParamModel` / `Vxn2Params` impls.

use std::sync::Arc;

use vxn2_app::{
    Controller, Layer, MatrixRow, NoopPresetStore, ParamId, UiEvent, ViewEvent, Vxn2Params,
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

/// Drain the view rx into a Vec for easier assertion.
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
fn matrix_row_custom_event_writes_model_and_echoes_view() {
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
            layer: Layer::Upper,
            slot: 3,
            row,
        })))
        .unwrap();

    tick_vxn2(&mut ctrl);

    // Model: row written through.
    let got = Vxn2Params::matrix_row(&*model, Layer::Upper, 3);
    assert_eq!(got, row);

    // Slot 3 is within the CLAP-automatable range (0..8), so depth also
    // landed in the CLAP table (upper-mtx4-depth, 1-based mtx# = slot+1).
    let clap_id = id_of("upper-mtx4-depth").expect("upper-mtx4-depth present");
    assert!((model.get(clap_id) - 0.42).abs() < 1e-5);

    // View: MatrixRowChanged echo present, with same payload.
    let events = drain(&view_rx);
    let mut saw = false;
    for ev in events {
        if let ViewEvent::Custom(payload) = ev {
            if let Ok(custom) = payload.downcast::<Vxn2ViewCustom>() {
                if let Vxn2ViewCustom::MatrixRowChanged {
                    layer: Layer::Upper,
                    slot: 3,
                    row: r,
                } = *custom
                {
                    assert_eq!(r, row);
                    saw = true;
                }
            }
        }
    }
    assert!(saw, "MatrixRowChanged echo not seen on view rx");
}

#[test]
fn edit_layer_custom_event_writes_and_echoes() {
    let (mut ctrl, view_rx, model) = build_controller();
    assert_eq!(Vxn2Params::edit_layer(&*model), Layer::Upper);

    ctrl.handle()
        .post(UiEvent::Custom(Box::new(Vxn2UiCustom::SetEditLayer {
            layer: Layer::Lower,
        })))
        .unwrap();

    tick_vxn2(&mut ctrl);

    assert_eq!(Vxn2Params::edit_layer(&*model), Layer::Lower);

    let saw_change = drain(&view_rx).into_iter().any(|ev| match ev {
        ViewEvent::Custom(p) => matches!(
            p.downcast::<Vxn2ViewCustom>().map(|b| *b),
            Ok(Vxn2ViewCustom::EditLayerChanged {
                layer: Layer::Lower
            })
        ),
        _ => false,
    });
    assert!(saw_change, "EditLayerChanged echo not seen");
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
            layer: Layer::Lower,
            slot: 10, // past the 0..8 CLAP-automatable range
            row,
        })))
        .unwrap();
    tick_vxn2(&mut ctrl);

    let got = Vxn2Params::matrix_row(&*model, Layer::Lower, 10);
    assert_eq!(got, row);
    // No CLAP id is affected by a slot 9-16 write — spot-check.
    let mtx1 = id_of("lower-mtx1-depth").unwrap();
    assert_eq!(model.get(mtx1), 0.0);
}
