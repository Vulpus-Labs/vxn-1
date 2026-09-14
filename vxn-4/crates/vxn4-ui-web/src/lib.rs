//! VXN4 HTML faceplate: bundles the page assets and hands them to
//! `vxn-core-ui-web`'s wry host (ticket 0387).
//!
//! The page is the port of `vxn-4/ui-mockup/index.html`, which stays in the
//! tree: it is a complete, interactive prototype of all three panels and it is
//! where layout iteration happens. A layout question gets answered there first
//! and ported here second, so the reviewed page and the shipped page cannot
//! drift apart by argument.
//!
//! ## What this crate does not do yet
//!
//! Bind anything. 0387 is the chrome — the editor opens under the host's
//! window, draws, closes and reopens — and every control on the page holds its
//! own value. There are no `parse_custom_ui` / `serialise_custom_view` hooks
//! here for the same reason: there is no vocabulary to parse until there is
//! something to say it to (0386's `vxn4-app`, then 0388's bindings).
//!
//! What the page *does* take from Rust is the handful of structural facts it
//! would otherwise have to restate — the operator count, the matrix slot
//! count, the curve table — spliced in as [`build_html`]'s config JSON. Those
//! are the numbers that would silently mis-index if the page and the engine
//! disagreed, so they have one owner.

use std::ffi::c_void;

use vxn_core_app::{ControllerHandle, CorpusHandle};
use vxn_core_matrix::curve::CURVE_LABELS;
// Re-exported so the clack shell can name the editor handle / error without
// depending on the shared crate directly.
use vxn_core_ui_web::{
    DEFAULT_MAX_BATCH_BYTES, WebEditorConfig, open_editor as core_open_editor, strip_esm_exports,
};
pub use vxn_core_ui_web::{EditorHandle, OpenEditorError};
use vxn4_dsp::ops::NOPS;
use vxn4_engine::{N_MACROS, N_MATRIX_SLOTS};

/// Logical pixel dimensions of the editor.
///
/// `--editor-w` in the stylesheet must agree with the width;
/// `editor_width_matches_the_css` asserts it rather than asking.
///
/// The height has no single CSS constant to check against — it is a sum, and
/// the sum is the whole decision. The faceplate is sized to its **tallest tab
/// pane**: the operators pane is 674px against the mixer's 494 and the
/// perform pane's 488, because it carries an extra row (the operator tab strip)
/// and its rows are taller. Sizing to the tallest and letting the other two
/// leave space at the bottom is vxn-1b's answer to the same question, and the
/// alternative — renegotiating `gui.get_size` on every tab change — makes the
/// host resize its plugin window under the player's hands while they are
/// working, which no host does gracefully and several do visibly.
///
/// ```text
///   10  padding-top                      674  tallest pane (--pane-h)
///   28  banner (26px + 1px borders)       10  padding-bottom
///    8  gap                              ───
///   30  preset bar (border-box)          803
///    8  gap
///   27  tab strip (26px tabs + 1px rule)
///    8  gap
/// ```
///
/// Trimming the operators pane instead was the other option the ticket offered
/// and was not taken: the 170px is three rows of real controls, and losing them
/// would be redesigning a settled design to save a window.
pub const EDITOR_WIDTH: u32 = 1140;
pub const EDITOR_HEIGHT: u32 = 803;

const HTML_TEMPLATE: &str = include_str!("../assets/index.html");
const STYLE_CSS: &str = include_str!("../assets/style.css");
const APP_JS: &str = include_str!("../assets/app.js");

/// The panel modules, in **splice order**.
///
/// Order is load-bearing and not alphabetical: the splice concatenates every
/// module into one inline `<script>` scope, where `const` bindings do not
/// hoist, so a module must follow everything it names at module level. `dial`
/// before `wave-knob` (the arc geometry), `combo` before `wave-picker`, panels
/// before `app`.
const PANEL_JS: [&str; 17] = [
    include_str!("../assets/panels/value.js"),
    include_str!("../assets/panels/pop.js"),
    include_str!("../assets/panels/dom.js"),
    include_str!("../assets/panels/drag.js"),
    include_str!("../assets/panels/dial.js"),
    include_str!("../assets/panels/fader.js"),
    include_str!("../assets/panels/toggle.js"),
    include_str!("../assets/panels/combo.js"),
    include_str!("../assets/panels/wave-knob.js"),
    include_str!("../assets/panels/wave-picker.js"),
    include_str!("../assets/panels/hfader.js"),
    include_str!("../assets/panels/meter.js"),
    include_str!("../assets/panels/canvas.js"),
    include_str!("../assets/panels/scope.js"),
    include_str!("../assets/panels/eg-graph.js"),
    include_str!("../assets/panels/ks-graph.js"),
    include_str!("../assets/panels/pm-grid.js"),
];

/// Open the VXN4 faceplate under `parent` (the raw NSView/HWND/xcb handle the
/// clack shell extracts in `gui::set_parent`).
///
/// Never panics — a bad parent or a wry build failure returns
/// [`OpenEditorError`], which the shell maps to `PluginError`. An unwind here
/// would cross the host's `extern "C"` frame, which is UB under
/// `panic = "unwind"`, so this path has no `unwrap` on it anywhere.
pub fn open_editor(
    parent: *mut c_void,
    ctrl: ControllerHandle,
    corpus: CorpusHandle,
) -> Result<EditorHandle, OpenEditorError> {
    let mut config = WebEditorConfig::new(build_html(), EDITOR_WIDTH, EDITOR_HEIGHT);
    config.max_batch_bytes = DEFAULT_MAX_BATCH_BYTES;
    // WebView2 user-data folder: `%LOCALAPPDATA%\Vulpus\VXN4\WebView2`. Avoids
    // the admin-only `C:\Program Files\<host>\<exe>.WebView2` default, which
    // fails the WebView2 env init with `E_ACCESSDENIED`.
    config.webview2_vendor = Some("Vulpus");
    config.webview2_product = Some("VXN4");
    core_open_editor(parent, ctrl, corpus, config)
}

/// The structural facts the page would otherwise restate. Everything here is a
/// count or a table the page **indexes into**, which is exactly the class of
/// constant where a disagreement is silent: a picker listing the curves in a
/// different order than `vxn_core_matrix` does is a wrong route, not an error.
fn config_json() -> String {
    serde_json::json!({
        "n_ops": NOPS,
        "n_macros": N_MACROS,
        "n_matrix_slots": N_MATRIX_SLOTS,
        "curves": CURVE_LABELS,
    })
    .to_string()
}

/// The page's whole JS, ESM markers stripped and joined in dependency order.
///
/// The three shared primitives come first: `valuePop` (the one popup element),
/// the cutoff/note helpers, and `wireDrag` (the pointer-capture choreography
/// every synth re-implemented until 0140 lifted it). This crate splices those
/// three individually rather than calling `shared_widgets_js()`, which would
/// also bring the mod-matrix curve picker — vxn-4's matrix curves are combos,
/// and shipping the picker's JS without its stylesheet is worse than not
/// shipping it.
fn faceplate_js() -> String {
    let mut parts = vec![
        strip_esm_exports(vxn_core_ui_web::VALUE_POP_JS),
        strip_esm_exports(vxn_core_ui_web::CUTOFF_TUNED_JS),
        strip_esm_exports(vxn_core_ui_web::WIRE_DRAG_JS),
    ];
    parts.extend(PANEL_JS.iter().map(|m| strip_esm_exports(m)));
    parts.push(strip_esm_exports(APP_JS));
    parts.join("\n;\n")
}

/// Splice CSS, the config JSON and the JS bundle into the HTML template.
///
/// The `.value-pop` ruleset comes from the shared crate rather than from
/// `style.css`: it was copied into three stylesheets once and drifted in all
/// three, and the page already uses it verbatim.
pub fn build_html() -> String {
    let css = format!("{}\n{}", STYLE_CSS, vxn_core_ui_web::VALUE_POP_CSS);
    HTML_TEMPLATE
        .replace("__CSS__", &css)
        .replace("__CONFIG_JSON__", &config_json())
        .replace("__APP_JS__", &faceplate_js())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vxn4_dsp::wavetable::Waveform;

    #[test]
    fn html_has_every_asset_spliced() {
        let html = build_html();
        assert!(!html.contains("__CSS__"));
        assert!(!html.contains("__APP_JS__"));
        assert!(!html.contains("__CONFIG_JSON__"));
        // One marker per bundle, so a dropped `include_str!` fails here rather
        // than as a blank panel in a host.
        assert!(html.contains(".pm-cell"), "stylesheet missing");
        assert!(html.contains(".value-pop"), "shared value-pop CSS missing");
        assert!(
            html.contains("function wireDrag"),
            "shared wireDrag missing"
        );
        assert!(html.contains("valuePop"), "shared value popup missing");
        assert!(
            html.contains("function createPmGrid"),
            "PM grid panel missing"
        );
        assert!(html.contains("VULPUS LABS"), "markup missing");
    }

    /// The ESM markers must be gone: `export` / `import` in a classic inline
    /// `<script>` is a `SyntaxError` that takes the whole page down, and the
    /// failure mode in a host is a blank editor with no console to read.
    #[test]
    fn the_bundle_carries_no_module_syntax() {
        for line in faceplate_js().lines() {
            let t = line.trim_start();
            assert!(
                !t.starts_with("export ") && !t.starts_with("import "),
                "module syntax survived the splice: {line}"
            );
        }
    }

    /// The host window's width and the stylesheet's must agree. They are set in
    /// different languages by different people, and a mismatch is a scroll bar
    /// or a strip of dead chrome — visible, but only if someone opens the
    /// editor.
    #[test]
    fn editor_width_matches_the_css() {
        const DECL: &str = "--editor-w: ";
        let at = STYLE_CSS.find(DECL).expect("no `--editor-w` in style.css");
        let tail = &STYLE_CSS[at + DECL.len()..];
        let value = &tail[..tail.find(';').expect("unterminated --editor-w")];
        let px: u32 = value
            .trim()
            .strip_suffix("px")
            .unwrap_or_else(|| panic!("--editor-w is `{value}`, not a px length"))
            .parse()
            .expect("--editor-w is not an integer");
        assert_eq!(px, EDITOR_WIDTH, "--editor-w and EDITOR_WIDTH disagree");
    }

    /// The height is the sum in [`EDITOR_HEIGHT`]'s doc comment, and `--pane-h`
    /// is the one term of it the stylesheet also states. Pin them together: a
    /// pane that grows a row is exactly the change that would otherwise clip
    /// the bottom panel's border and nothing else.
    #[test]
    fn editor_height_accounts_for_the_tallest_pane() {
        const DECL: &str = "--pane-h: ";
        let at = STYLE_CSS.find(DECL).expect("no `--pane-h` in style.css");
        let tail = &STYLE_CSS[at + DECL.len()..];
        let value = &tail[..tail.find(';').expect("unterminated --pane-h")];
        let pane: u32 = value
            .trim()
            .strip_suffix("px")
            .expect("px")
            .parse()
            .expect("int");
        // padding + banner + gap + preset bar + gap + tab strip + gap, per the
        // table on EDITOR_HEIGHT.
        const CHROME: u32 = 10 + 28 + 8 + 30 + 8 + 27 + 8 + 10;
        assert_eq!(pane + CHROME, EDITOR_HEIGHT);
    }

    /// The page's config is the engine's numbers, not a copy of them.
    #[test]
    fn config_ships_the_engine_geometry() {
        let cfg = config_json();
        assert!(cfg.contains(&format!("\"n_ops\":{NOPS}")));
        assert!(cfg.contains(&format!("\"n_matrix_slots\":{N_MATRIX_SLOTS}")));
        assert!(cfg.contains(&format!("\"n_macros\":{N_MACROS}")));
        // The curve list is indexed by the flat (polarity, shape) code a slot
        // stores, so its ORDER is the contract, not just its contents.
        assert!(cfg.contains("[\"Lin\",\"Exp\",\"Log\",\"Bipolar\""));
    }

    /// The waveform picker draws eleven shapes: the four the engine has, then
    /// seven proposed ones that 0388 greys. The first four must stay the
    /// engine's four **in table order** — the picker's index is the table
    /// index, so a reordering here would select the wrong wave silently.
    #[test]
    fn the_wave_picker_leads_with_the_engine_waveforms() {
        let js = include_str!("../assets/panels/wave-picker.js");
        let mut cursor = 0;
        for w in Waveform::ALL {
            let needle = format!("{{ name: \"{w:?}\"");
            let at = js[cursor..]
                .find(&needle)
                .unwrap_or_else(|| panic!("{w:?} missing from OP_WAVE_DEFS, or out of order"));
            cursor += at + needle.len();
        }
    }

    /// A host that hands over a null parent must get an error back, not an
    /// unwind. `gui.set_parent` is called across the host's `extern "C"`
    /// frame, where a panic is UB under `panic = "unwind"` — so this is the
    /// difference between a plugin that refuses to show an editor and a host
    /// that disappears. The check happens before wry is touched, which is also
    /// what makes this test safe to run off a UI thread.
    #[test]
    fn a_null_parent_is_an_error_and_not_a_panic() {
        let corpus = std::sync::Arc::new(std::sync::Mutex::new(Default::default()));
        // `EditorHandle` is not `Debug`, so this matches rather than unwraps.
        match open_editor(std::ptr::null_mut(), ControllerHandle::detached(), corpus) {
            Err(OpenEditorError::BadParent(_)) => {}
            Err(other) => panic!("wrong error: {other}"),
            Ok(_) => panic!("a null parent produced a WebView"),
        }
    }

    /// The vitest suite over the panels' pure logic — the PM grid's sign
    /// encoding, the waveform table's DC flags, the taper round-trip, the
    /// key-scaling curve. Gated behind an env var so `cargo test` on a machine
    /// with no `node_modules` is a pass rather than a confusing failure; the
    /// same shape vxn-1b and vxn-2 use.
    #[test]
    fn js_suite_passes() {
        if std::env::var("VXN_JS_TESTS").is_err() {
            eprintln!(
                "VXN_JS_TESTS unset; skipping JS suite. \
                 Run `VXN_JS_TESTS=1 cargo test -p vxn4-ui-web` to enable."
            );
            return;
        }
        let status = std::process::Command::new("npm")
            .args(["test", "--silent"])
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"))
            .status()
            .expect("npm not found — install Node 20+ or unset VXN_JS_TESTS");
        assert!(status.success(), "JS suite failed under `npm test`");
    }
}
