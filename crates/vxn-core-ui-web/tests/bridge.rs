//! Bridge tests: parse_ui_event for the shared opcodes, batch_chunks
//! dedup + byte cap, corpus snapshot shape.

use std::path::PathBuf;

use vxn_core_app::{
    ParamId, PresetCorpus, PresetMeta, PresetSource, UiEvent, UserFolderEntry, UserPresetEntry,
    ViewEvent,
};
use vxn_core_ui_web::{
    batch_chunks, corpus_snapshot_json, parse_ui_event, parse_ui_event_default,
    view_event_to_json,
};

#[test]
fn parse_set_param_norm() {
    let ev = parse_ui_event(
        r#"{"op":"set_param_norm","id":42,"norm":0.5}"#,
        None,
    )
    .unwrap();
    match ev {
        UiEvent::SetParamNorm { id, norm } => {
            assert_eq!(id.raw(), 42);
            assert!((norm - 0.5).abs() < 1e-6);
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn parse_factory_load() {
    let ev = parse_ui_event(r#"{"op":"load_factory","index":7}"#, None).unwrap();
    match ev {
        UiEvent::LoadPreset { source: PresetSource::Factory { index } } => {
            assert_eq!(index, 7);
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn parse_user_mutation_ops() {
    let ev = parse_ui_event(
        r#"{"op":"rename_preset","path":"/u/x.preset","new_name":"Y"}"#,
        None,
    )
    .unwrap();
    match ev {
        UiEvent::RenamePreset { path, new_name } => {
            assert_eq!(path, PathBuf::from("/u/x.preset"));
            assert_eq!(new_name, "Y");
        }
        other => panic!("wrong variant: {other:?}"),
    }
    let ev = parse_ui_event(r#"{"op":"delete_preset","path":"/u/x.preset"}"#, None).unwrap();
    assert!(matches!(ev, UiEvent::DeletePreset { .. }));
}

#[test]
fn parse_step_preset() {
    let ev = parse_ui_event(r#"{"op":"step_preset","delta":-1}"#, None).unwrap();
    match ev {
        UiEvent::StepPreset { delta } => assert_eq!(delta, -1),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn unknown_opcode_falls_through_to_custom_hook() {
    use std::sync::Arc;
    let custom = Arc::new(|op: &str, _v: &serde_json::Value| {
        if op == "vxn1_set_key_mode" {
            Some(UiEvent::Custom(Box::new("seen".to_string())))
        } else {
            None
        }
    }) as vxn_core_ui_web::ParseCustomUi;
    let ev = parse_ui_event(
        r#"{"op":"vxn1_set_key_mode","mode":2}"#,
        Some(&custom),
    )
    .unwrap();
    match ev {
        UiEvent::Custom(payload) => {
            let s = payload.downcast::<String>().unwrap();
            assert_eq!(*s, "seen");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn parse_ui_event_default_returns_none_for_unknown() {
    let v: serde_json::Value =
        serde_json::from_str(r#"{"op":"unknown","x":1}"#).unwrap();
    assert!(parse_ui_event_default("unknown", &v).is_none());
}

#[test]
fn batch_chunks_dedupes_param_changes() {
    // Ticket gate: 1000 events on 10 distinct ParamIds → ≤ 10 output
    // chunks. With dedup that's 10 deduped events; well inside the
    // 100 KB default cap so it should be a single chunk.
    let mut events: Vec<ViewEvent> = Vec::with_capacity(1000);
    for i in 0..1000 {
        events.push(ViewEvent::ParamChanged {
            id: ParamId::new(i % 10),
            plain: i as f32,
            norm: (i % 10) as f32 / 10.0,
            display: format!("{i}"),
        });
    }
    let chunks = batch_chunks(&events, 100_000, None);
    assert!(
        chunks.len() <= 10,
        "expected <= 10 chunks, got {}",
        chunks.len()
    );
    assert_eq!(chunks.len(), 1, "actual run packs into a single chunk");
    let one = &chunks[0];
    // Each id appears exactly once.
    for i in 0..10 {
        let needle = format!("\"id\":{i},");
        assert!(
            one.contains(&needle),
            "missing id {i} in deduped chunk: {one}"
        );
    }
}

#[test]
fn batch_chunks_splits_at_byte_cap() {
    // Force the splitter: 8 distinct ids, each carrying a fat display
    // string. Cap at a value well under the total so we get >1 chunk.
    let big = "x".repeat(200);
    let mut events: Vec<ViewEvent> = Vec::new();
    for i in 0..8 {
        events.push(ViewEvent::ParamChanged {
            id: ParamId::new(i),
            plain: 0.0,
            norm: 0.0,
            display: big.clone(),
        });
    }
    let chunks = batch_chunks(&events, 500, None);
    assert!(chunks.len() > 1, "expected split, got {} chunks", chunks.len());
    for c in &chunks {
        assert!(c.starts_with('['));
        assert!(c.ends_with(']'));
    }
}

#[test]
fn batch_chunks_empty_yields_empty() {
    let chunks = batch_chunks(&[], 100_000, None);
    assert!(chunks.is_empty());
}

#[test]
fn view_event_serialises_param_changed() {
    let ev = ViewEvent::ParamChanged {
        id: ParamId::new(3),
        plain: 0.25,
        norm: 0.5,
        display: "0.25".to_string(),
    };
    let s = view_event_to_json(&ev, None).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["kind"], "param_changed");
    assert_eq!(v["id"], 3);
}

#[test]
fn corpus_snapshot_groups_by_category() {
    let corpus = PresetCorpus {
        factory: vec![
            PresetMeta { name: "A".into(), category: Some("Bass".into()), ..Default::default() },
            PresetMeta { name: "B".into(), category: Some("Bass".into()), ..Default::default() },
            PresetMeta { name: "C".into(), category: None, ..Default::default() },
        ],
        user: vec![UserFolderEntry {
            name: None,
            presets: vec![UserPresetEntry {
                path: PathBuf::from("/u/p.preset"),
                meta: PresetMeta { name: "P".into(), ..Default::default() },
                folder: None,
            }],
        }],
    };
    let json = corpus_snapshot_json(&corpus, "Uncategorised");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let factory = v["factory"].as_array().unwrap();
    let cats: Vec<&str> = factory
        .iter()
        .map(|g| g["category"].as_str().unwrap())
        .collect();
    assert!(cats.contains(&"Bass"));
    assert!(cats.contains(&"Uncategorised"));
    let user = v["user"].as_array().unwrap();
    assert_eq!(user.len(), 1);
    let user_presets = user[0]["presets"].as_array().unwrap();
    assert_eq!(user_presets[0]["name"], "P");
}
