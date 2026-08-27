//! The page's custom-op vocabulary — the one place the wire *names* live (0316).
//!
//! Three surfaces speak it and they used to transcribe it independently:
//! `vxn1b-ui-web` decoded the strings for the native editor, `vxn1b-web-controller`
//! decoded the same distinctions as ordinals for the browser, and
//! `faceplate-bridge.mjs` carried a third copy translating strings to ordinals.
//! Three writings of one enum ordering, with nothing comparing them.
//!
//! The two Rust decoders now both read the tables below. The JS one cannot —
//! it is a different language in a different memory space — so it is *pinned*
//! instead: `vxn1b-web-controller` publishes [`vocab_json`] through
//! `vxnc_vocab_json_ptr/len`, and `vxn1b-wasm/web/vocab-agreement.test.mjs`
//! asserts the page's tables against it out of the BUILT artifact. Drift fails
//! a test rather than being forbidden by a comment.
//!
//! These are **wire names, not display labels.** They appear in saved-nothing
//! and shown-nowhere; renaming one silently drops an opcode, which is why they
//! are worth pinning and why nothing here should be "tidied" to match UI copy.

use crate::engine::MatrixField;
use crate::params::Layer;
use crate::scope::ScopeTap;

/// Layer side: the two names the page uses for [`Layer`].
///
/// "upper"/"lower" rather than "l1"/"l2" because the faceplate stacks them.
pub const LAYER_NAMES: [(&str, Layer); 2] = [("upper", Layer::L1), ("lower", Layer::L2)];

/// Matrix topology field, and the ordinal it travels as on the browser wire.
///
/// The ordinal is the array position, so this table defines both mappings and
/// they cannot disagree.
pub const MATRIX_FIELD_NAMES: [(&str, MatrixField); 4] = [
    ("source", MatrixField::Source),
    ("dest", MatrixField::Dest),
    ("curve", MatrixField::Curve),
    ("scale", MatrixField::ScaleSrc),
];

/// Oscilloscope tap. `off` is what the page sends when the scope is not
/// showing, and it is a real member of the vocabulary rather than an absence.
pub const SCOPE_TAP_NAMES: [(&str, ScopeTap); 3] = [
    ("off", ScopeTap::Off),
    ("upper", ScopeTap::Layer1),
    ("lower", ScopeTap::Layer2),
];

/// Decode a layer side. `None` for an unknown name — the opcode is dropped
/// rather than defaulting to Layer 1, which would edit the wrong layer.
#[inline]
pub fn layer_from_name(name: &str) -> Option<Layer> {
    LAYER_NAMES.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

/// Decode a matrix topology field by name. `None` for an unknown name — a bad
/// selector must not silently rewrite the wrong field.
#[inline]
pub fn matrix_field_from_name(name: &str) -> Option<MatrixField> {
    MATRIX_FIELD_NAMES.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

/// Decode a matrix topology field by its wire ordinal (the browser path, where
/// the C ABI carries a `u32` rather than a string). Same table, so the two
/// decoders cannot disagree about which index means which field.
#[inline]
pub fn matrix_field_from_wire(index: u32) -> Option<MatrixField> {
    MATRIX_FIELD_NAMES.get(index as usize).map(|(_, v)| *v)
}

/// Decode a scope tap by name. `None` for an unknown name.
#[inline]
pub fn scope_tap_from_name(name: &str) -> Option<ScopeTap> {
    SCOPE_TAP_NAMES.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

/// The whole vocabulary as JSON, for the JS half to assert itself against.
///
/// Names map to the ordinal the browser wire carries, which for layers and
/// scope taps is the enum discriminant and for matrix fields is the table
/// position. Also carries the matrix geometry and the default split point, so
/// the page has one place to check every constant it mirrors.
pub fn vocab_json() -> String {
    let pairs = |entries: &[(&str, u32)]| -> String {
        entries
            .iter()
            .map(|(n, v)| format!("\"{n}\":{v}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let layers: Vec<(&str, u32)> = LAYER_NAMES.iter().map(|(n, l)| (*n, *l as u32)).collect();
    let fields: Vec<(&str, u32)> = MATRIX_FIELD_NAMES
        .iter()
        .enumerate()
        .map(|(i, (n, _))| (*n, i as u32))
        .collect();
    let taps: Vec<(&str, u32)> = SCOPE_TAP_NAMES
        .iter()
        .map(|(n, t)| (*n, t.code() as u32))
        .collect();
    format!(
        "{{\"layer\":{{{}}},\"matrixField\":{{{}}},\"scopeTap\":{{{}}},\
         \"matrixSlots\":{},\"layerCount\":{},\"splitPoint\":{{\"min\":{},\"max\":{},\"default\":{}}}}}",
        pairs(&layers),
        pairs(&fields),
        pairs(&taps),
        crate::matrix::N_SLOTS,
        Layer::ALL.len(),
        SPLIT_POINT_MIN,
        SPLIT_POINT_MAX,
        crate::engine::DEFAULT_SPLIT_POINT,
    )
}

/// Lowest note the split-point slider offers (C0). The engine accepts any MIDI
/// note; this is the UI's range, published here so the page stops carrying its
/// own copy in three places.
pub const SPLIT_POINT_MIN: u8 = 12;
/// Highest note the split-point slider offers (C7).
pub const SPLIT_POINT_MAX: u8 = 96;

#[cfg(test)]
mod tests {
    use super::*;

    /// Names decode, and unknown names are dropped rather than guessed at — the
    /// distinction that keeps a typo'd opcode from editing the wrong thing.
    #[test]
    fn names_round_trip_and_unknowns_are_dropped() {
        for (name, want) in LAYER_NAMES {
            assert_eq!(layer_from_name(name), Some(want));
        }
        for (name, want) in MATRIX_FIELD_NAMES {
            assert_eq!(matrix_field_from_name(name), Some(want));
        }
        for (name, want) in SCOPE_TAP_NAMES {
            assert_eq!(scope_tap_from_name(name), Some(want));
        }
        assert_eq!(layer_from_name("middle"), None);
        assert_eq!(matrix_field_from_name("depth"), None, "depth is a param, not topology");
        assert_eq!(scope_tap_from_name(""), None);
    }

    /// The ordinal decoder and the name decoder are the same table read two
    /// ways, so index `i` must be the field `MATRIX_FIELD_NAMES[i]` names.
    #[test]
    fn the_wire_ordinal_agrees_with_the_name() {
        for (i, (name, want)) in MATRIX_FIELD_NAMES.iter().enumerate() {
            assert_eq!(matrix_field_from_wire(i as u32), Some(*want), "index {i} ({name})");
            assert_eq!(matrix_field_from_name(name), Some(*want));
        }
        assert_eq!(matrix_field_from_wire(MATRIX_FIELD_NAMES.len() as u32), None);
    }

    /// The JSON the JS half asserts itself against. Pinned literally: if this
    /// changes, `vocab-agreement.test.mjs` is what has to change with it.
    #[test]
    fn vocab_json_has_the_shape_the_page_expects() {
        let j = vocab_json();
        for frag in [
            "\"layer\":{\"upper\":0,\"lower\":1}",
            "\"matrixField\":{\"source\":0,\"dest\":1,\"curve\":2,\"scale\":3}",
            "\"scopeTap\":{\"off\":0,\"upper\":1,\"lower\":2}",
            "\"matrixSlots\":16",
            "\"layerCount\":2",
            "\"splitPoint\":{\"min\":12,\"max\":96,\"default\":60}",
        ] {
            assert!(j.contains(frag), "vocab JSON missing {frag}\n  got: {j}");
        }
    }

    #[test]
    fn the_split_range_contains_the_default() {
        assert!(SPLIT_POINT_MIN <= crate::engine::DEFAULT_SPLIT_POINT);
        assert!(crate::engine::DEFAULT_SPLIT_POINT <= SPLIT_POINT_MAX);
    }
}
