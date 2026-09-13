//! VXN4's `clap.state` blob (v2 — ticket 0384).
//!
//! ```text
//! magic       : b"VX4S"            (4 bytes)
//! version     : u16 LE             (= 2)
//! n_params    : u16 LE             (= TOTAL_PARAMS at save time)
//! values      : f32 LE x n_params
//! payload_len : u32 LE             \ v2 only
//! payload     : UTF-8 TOML         /
//! ```
//!
//! v1 was the header and the values, and at the time that genuinely was all the
//! user state there was: the patches were hardwired, so a project restored to a
//! known patch plus eleven knob positions. Patches are editable state owned by
//! the main thread now ([`vxn4_engine::shared`]), so the blob carries one.
//!
//! ## Two compatibility schemes, covering disjoint halves
//!
//! The **values** are positional, and forward-compatible because `n_params` is
//! written rather than assumed: a blob from a build with fewer params loads into
//! one with more (the rest keep their defaults), and one with more loads into a
//! build with fewer (the tail is read and skipped, or the stream desynchronises).
//! That is sound only while ids are **never reused for a different meaning** —
//! the invariant belongs to [`crate::params::decode`] and its
//! `the_positional_scheme_is_stable` test is what holds it.
//!
//! The **payload** has no positional invariant and never will. It is
//! [`vxn4_engine::preset`]'s sparse TOML, keyed by descriptor name: a field the
//! file omits takes its descriptor default, and a field this build has never
//! heard of is skipped. Compatibility runs both directions for free, which is
//! the property a host blob most needs and the one hardest to retrofit onto a
//! binary layout. The two schemes are unrelated and neither constrains the
//! other; the eleven positional ids are not an argument about the payload.
//!
//! Text rather than a second binary encoding, deliberately. It is the format the
//! preset files already speak, so there is one codec to write and one to test; a
//! blob that fails to load can be read by a person; and size is not a constraint
//! — a sparse patch is a few kilobytes and hosts store far larger blobs
//! routinely.
//!
//! ## Who owns the patch index
//!
//! Both a patch **selection** and a patch **body** are in the blob, and they are
//! authoritative for different things.
//!
//! **The CLAP `patch` parameter owns the selection.** It is an automatable host
//! param with a lane in the project, the host may write it at any time, and
//! `get_value` has to answer for it on demand — so the host is its authority and
//! the plugin is not. Moving it into the store would mean the store answering
//! `get_value` for all eleven params, which is 0386's rewiring and deliberately
//! not this ticket's.
//!
//! It is also written on the **audio thread**, as a param event inside
//! `process`. The store's bulk install takes a main-thread-only mutex *by
//! construction* ([`SharedParams`]), so the audio thread cannot be the side that
//! moves the store — not by convention, but because there is no call it could
//! make. Any design where the selection moves the body has to join them
//! somewhere the main thread is running.
//!
//! **The store owns the body**, and the selection names the factory entry that
//! body started from. [`rebase_selection`] is the join: it adopts a selection
//! the store has not seen yet, and the store's own `patch` cell records which
//! one it last adopted. A re-base therefore happens when the *selection* moves
//! and never because the body drifted from it — which is what keeps the rule
//! correct once the faceplate can edit a field and the two stop being equal.
//!
//! This ticket has exactly two main-thread moments to hang that on, [`save`] and
//! [`load`], and they are also the only two places that read the store's body,
//! so the join does not have to be continuous and is not. 0386 and 0387 give the
//! shell a controller and an editor tick; it moves there.
//!
//! ## A v1 blob still opens a project
//!
//! Accepted rather than merely tolerated: the values restore exactly as before,
//! and the patch resolves to the factory entry the stored index names. That is
//! what a v1 blob *meant* — a patch of that era could not be anything else — so
//! nothing is being guessed at.
//!
//! ## All or nothing, and one crossing
//!
//! The blob is parsed in full before a single value is committed. A truncated
//! stream, a payload that is not UTF-8, and a payload that is not a preset all
//! fail the load outright and leave the cache and the store exactly as they
//! were. A project that half-opens is worse than one that refuses to: the first
//! looks like a synth bug for the rest of the session.
//!
//! The restore then crosses to the audio thread the way every bulk change does —
//! values, then **one** topology snapshot, then the reload flag — so the
//! renderer sees one edit rather than a stream of field writes, and cannot
//! observe it half applied whichever of the producer's stores it happens to see
//! first. [`vxn4_engine::shared`] carries that argument in full.
//!
//! Save is deterministic, which `clap-validator` checks by saving twice and
//! comparing: the values come out of the cache in id order, and the payload's
//! tables are ordered maps written in slot order.

use vxn4_engine::params::param_for_clap;
use vxn4_engine::{Macros, Meta, ParamId, SharedParams, patch_names, read_preset, write_preset};

use crate::params::{self, ParamCache, TOTAL_PARAMS};

const MAGIC: [u8; 4] = *b"VX4S";
const VERSION: u16 = 2;

/// The `clap_id` of the patch selector. Named rather than spelled `0`, because
/// everything below turns on it being that one param and a bare zero next to a
/// descriptor id would read as either id space.
const PATCH_CLAP_ID: usize = 0;

/// Where the store records which factory entry its body was last based on.
///
/// The descriptor table already has a `patch` cell in its host region and
/// nothing else writes it, so the bookkeeping needs no field of its own — and
/// it lives next to the body it describes rather than in a second structure
/// that could get out of step with it.
fn selection_id() -> ParamId {
    param_for_clap(PATCH_CLAP_ID).expect("the patch selector is a host param")
}

/// Adopt the factory entry the host's `patch` parameter names, if the store has
/// not adopted it already. Returns the selection either way.
///
/// **Main thread.** See the module docs for why this is a re-base on the
/// selection moving rather than a mirror of it: a body that has been edited away
/// from its factory entry must survive, and only a *new* selection may replace
/// it.
pub fn rebase_selection(cache: &ParamCache, store: &SharedParams) -> usize {
    let want = params::patch_from(cache.get(PATCH_CLAP_ID));
    if params::patch_from(store.get(selection_id())) != want {
        store.load_factory(want);
        store.set(selection_id(), want as f32);
    }
    want
}

/// Serialize the cache and the store. Deterministic — the same state always
/// produces identical bytes.
///
/// `Err` only if the patch cannot be serialised, which the preset codec argues
/// it cannot: every value is clamped finite and every label is a static string.
/// It is a `Result` anyway because the alternative on that impossible path is
/// writing a blob with no payload, and *that* blob restores a patch of
/// descriptor defaults — a silently wrong sound where a refused save is a
/// visible failure.
#[allow(clippy::result_unit_err)] // failure sentinel; the shell maps it
pub fn save(cache: &ParamCache, store: &SharedParams) -> Result<Vec<u8>, ()> {
    let index = rebase_selection(cache, store);
    let meta = Meta {
        name: patch_names()[index].to_string(),
        ..Meta::default()
    };
    let text = write_preset(&meta, &store.patch_snapshot(), &Macros::default()).map_err(|_| ())?;

    let mut b = Vec::with_capacity(12 + TOTAL_PARAMS * 4 + text.len());
    b.extend_from_slice(&MAGIC);
    b.extend_from_slice(&VERSION.to_le_bytes());
    b.extend_from_slice(&(TOTAL_PARAMS as u16).to_le_bytes());
    for id in 0..TOTAL_PARAMS {
        b.extend_from_slice(&cache.get(id).to_le_bytes());
    }
    b.extend_from_slice(&(text.len() as u32).to_le_bytes());
    b.extend_from_slice(text.as_bytes());
    Ok(b)
}

/// Restore into `cache` and `store`. `Err` on bad magic, a future version, a
/// truncated stream or an unparseable payload — the shell maps that to a failed
/// `clap_plugin_state::load`.
///
/// Nothing is written until the whole blob has parsed, so a rejected load leaves
/// both sides untouched rather than half restored.
///
/// Values go through [`ParamCache::set`] and the patch through the descriptor
/// clamps, so a corrupted blob cannot put a patch index out of range or hand the
/// engine a nonsense number.
#[allow(clippy::result_unit_err)] // parse-failure sentinel; the shell maps it
pub fn load(bytes: &[u8], cache: &ParamCache, store: &SharedParams) -> Result<(), ()> {
    let mut r = Reader { b: bytes, pos: 0 };
    if r.take(4)? != MAGIC {
        return Err(());
    }
    let version = r.u16()?;
    if version > VERSION {
        return Err(());
    }
    let n = r.u16()? as usize;
    let mut values = Vec::with_capacity(n.min(TOTAL_PARAMS));
    for id in 0..n {
        let v = f32::from_le_bytes(r.take(4)?.try_into().map_err(|_| ())?);
        // Ids past our table are skipped, not an error — see the module note on
        // the positional scheme. The read still has to happen, or the stream
        // desynchronises and the payload length lands on a float.
        if id < TOTAL_PARAMS {
            values.push(v);
        }
    }

    let patch = if version >= 2 {
        let len = r.u32()? as usize;
        let text = std::str::from_utf8(r.take(len)?).map_err(|_| ())?;
        // Warnings are dropped rather than surfaced: they name fields a *newer*
        // build wrote, the shell has nowhere to put a string, and the load is
        // still the best reading of the blob available. A preset file gets its
        // warnings shown because a person chose that file; a project's own blob
        // is not something the player picked.
        Some(read_preset(text).map_err(|_| ())?.patch)
    } else {
        None
    };
    // Anything past the payload belongs to a version that is not this one and
    // is ignored, which is the same courtesy the value tail gets.

    // Parsed. Only now does anything move.
    for (id, v) in values.iter().enumerate() {
        cache.set(id, *v);
    }
    let index = params::patch_from(cache.get(PATCH_CLAP_ID));
    match patch {
        Some(p) => store.install(&p.into()),
        // v1: the blob carries no body, and the index is a complete description
        // of the patch it was saved from.
        None => store.load_factory(index),
    }
    store.set(selection_id(), index as f32);
    Ok(())
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ()> {
        let end = self.pos.checked_add(n).ok_or(())?;
        let s = self.b.get(self.pos..end).ok_or(())?;
        self.pos = end;
        Ok(s)
    }

    fn u16(&mut self) -> Result<u16, ()> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().map_err(|_| ())?,
        ))
    }

    fn u32(&mut self) -> Result<u32, ()> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| ())?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{N_FIXED, default_value};
    use vxn4_engine::{Engine, SlotEdit, SlotField, SourceId, desc, id_for_name, patch, patch_ids};

    fn filled() -> ParamCache {
        let c = ParamCache::new();
        c.set(0, 3.0); // patch: saws
        c.set(1, 1.0); // quality: 16x
        c.set(2, 0.75); // master gain
        for m in 0..vxn4_engine::N_MACROS {
            c.set(N_FIXED + m, 0.1 * m as f32);
        }
        c
    }

    /// A store holding whatever the cache's selection names, as the shell's own
    /// save path would have left it.
    fn store_for(cache: &ParamCache) -> SharedParams {
        let s = SharedParams::new();
        rebase_selection(cache, &s);
        s
    }

    /// Every patch value, and the topology, compared through
    /// [`SharedParams::patch_snapshot`].
    ///
    /// Through the snapshot rather than `matrix_snapshot` because a slot's depth
    /// has two homes and only one authority: an edited store's mirror carries
    /// whatever depth the table held when it was installed, and
    /// `matrix-NN-depth` is the value that means anything. Comparing the raw
    /// mirrors would fail on a difference that is by design.
    fn assert_same_patch(a: &SharedParams, b: &SharedParams, what: &str) {
        for id in patch_ids() {
            assert_eq!(
                a.get(id).to_bits(),
                b.get(id).to_bits(),
                "{what}: {}",
                desc(id).unwrap().name
            );
        }
        assert_eq!(
            a.patch_snapshot().matrix,
            b.patch_snapshot().matrix,
            "{what}: topology"
        );
    }

    /// A blob assembled by hand, so a test can write a header this build would
    /// never produce.
    fn blob_with(cache: &ParamCache, version: u16, n: usize, payload: Option<&str>) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&MAGIC);
        b.extend_from_slice(&version.to_le_bytes());
        b.extend_from_slice(&(n as u16).to_le_bytes());
        for id in 0..n {
            let v = if id < TOTAL_PARAMS {
                cache.get(id)
            } else {
                9.5
            };
            b.extend_from_slice(&v.to_le_bytes());
        }
        if let Some(text) = payload {
            b.extend_from_slice(&(text.len() as u32).to_le_bytes());
            b.extend_from_slice(text.as_bytes());
        }
        b
    }

    // ── the eleven params ───────────────────────────────────────────────────

    #[test]
    fn a_blob_round_trips_every_param() {
        let a = filled();
        let blob = save(&a, &store_for(&a)).expect("save");
        let (b, s) = (ParamCache::new(), SharedParams::new());
        load(&blob, &b, &s).expect("load");
        for id in 0..TOTAL_PARAMS {
            assert_eq!(a.get(id), b.get(id), "param {id}");
        }
    }

    /// Byte-for-byte determinism: `clap-validator` saves twice and compares.
    #[test]
    fn saving_the_same_state_twice_gives_the_same_bytes() {
        let c = filled();
        let s = store_for(&c);
        assert_eq!(save(&c, &s), save(&c, &s));
        // And across two stores that reached the same state independently, so
        // this is a property of the state rather than of one object's history.
        assert_eq!(save(&c, &s), save(&c, &store_for(&c)));
    }

    #[test]
    fn the_blob_is_the_shape_the_header_claims() {
        let c = ParamCache::new();
        let blob = save(&c, &store_for(&c)).expect("save");
        assert_eq!(&blob[..4], b"VX4S");
        assert_eq!(u16::from_le_bytes(blob[4..6].try_into().unwrap()), 2);
        assert_eq!(
            u16::from_le_bytes(blob[6..8].try_into().unwrap()) as usize,
            TOTAL_PARAMS
        );
        let at = 8 + TOTAL_PARAMS * 4;
        let len = u32::from_le_bytes(blob[at..at + 4].try_into().unwrap()) as usize;
        assert_eq!(
            blob.len(),
            at + 4 + len,
            "the payload is not the length claimed"
        );
        assert!(len > 0, "v2 must carry a payload");
    }

    // ── the payload ─────────────────────────────────────────────────────────

    /// The whole ticket: the patch is in the blob, field for field and route for
    /// route, not merely the index that names it.
    #[test]
    fn the_patch_crosses_the_blob_field_by_field() {
        for p in 0..vxn4_engine::N_PATCHES {
            let c = ParamCache::new();
            c.set(0, p as f32);
            let src = store_for(&c);
            let blob = save(&c, &src).expect("save");

            let (c2, dst) = (ParamCache::new(), SharedParams::new());
            load(&blob, &c2, &dst).expect("load");
            assert_same_patch(&src, &dst, patch_names()[p]);
        }
    }

    /// An **edited** patch — one the selection cannot name — has to survive, or
    /// the payload is decorative and the index is still doing all the work.
    #[test]
    fn an_edited_patch_survives_the_blob() {
        let c = filled(); // selection: saws
        let src = store_for(&c);
        let ratio = id_for_name("op-0-ratio").expect("a patch field");
        src.set(ratio, 3.5);
        src.set(id_for_name("matrix-04-depth").unwrap(), -0.625);
        src.edit_slot(SlotEdit {
            slot: 4,
            field: SlotField::Source,
            value: SourceId::Macro7 as u8,
        });

        let blob = save(&c, &src).expect("save");
        let (c2, dst) = (ParamCache::new(), SharedParams::new());
        load(&blob, &c2, &dst).expect("load");

        assert_eq!(dst.get(ratio), 3.5);
        assert_eq!(dst.matrix_snapshot().slots[4].source, SourceId::Macro7);
        assert_same_patch(&src, &dst, "edited");
        // ...and the edit is not quietly re-based away by the next save, which
        // is the half of the ownership rule a mirror would get wrong.
        assert_eq!(save(&c, &dst).expect("save"), blob);
    }

    /// The restore reaches the renderer as one record, not as 48 slot edits,
    /// and the values ride with it.
    #[test]
    fn a_restore_crosses_as_one_snapshot() {
        let c = filled();
        let blob = save(&c, &store_for(&c)).expect("save");

        let (c2, dst) = (ParamCache::new(), SharedParams::new());
        let mut e = Engine::new(48_000.0);
        dst.sync(&mut e); // a fresh pair agrees; nothing is owed
        assert_eq!(dst.topology_backlog(), 0);

        load(&blob, &c2, &dst).expect("load");
        assert_eq!(dst.topology_backlog(), 1, "one record for the whole patch");
        assert!(dst.sync(&mut e), "the restore never reached the engine");
        assert_eq!(*e.matrix(), dst.matrix_snapshot());
        assert_eq!(
            e.patch_name(),
            "sine",
            "set_patch is the CLAP path, not this"
        );
    }

    /// A restored patch renders as the patch it was, which is a stronger claim
    /// than every field matching — a field the codec never looks at would pass
    /// the comparison and fail here.
    #[test]
    fn a_restored_patch_renders_like_the_one_that_was_saved() {
        let render = |sp: &SharedParams| {
            let mut e = Engine::new(48_000.0);
            sp.request_topology_resync();
            sp.sync(&mut e);
            for n in [48u8, 60, 67] {
                e.note_on(n, 100);
            }
            let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
            let mut out = Vec::new();
            for _ in 0..12 {
                e.process(&mut l, &mut r);
                out.extend(l.iter().chain(r.iter()).map(|s| s.to_bits()));
            }
            out
        };

        for p in 0..vxn4_engine::N_PATCHES {
            let c = ParamCache::new();
            c.set(0, p as f32);
            let src = store_for(&c);
            let blob = save(&c, &src).expect("save");
            let (c2, dst) = (ParamCache::new(), SharedParams::new());
            load(&blob, &c2, &dst).expect("load");
            assert_eq!(render(&dst), render(&src), "{} changed", patch_names()[p]);
        }
    }

    /// Forward and backward compatibility, which for a name-keyed payload are
    /// the same mechanism seen from two sides: a key this build does not have is
    /// skipped, and a field the file does not mention takes its default.
    #[test]
    fn a_payload_from_a_different_build_loads_either_way() {
        let text = "schema = 1\n[meta]\nname = \"X\"\n\
                    [params]\nop-0-ratio = 3.5\nfilter-cutoff = 800.0\n";
        let c = filled();
        let blob = blob_with(&c, 2, TOTAL_PARAMS, Some(text));

        let (c2, s) = (ParamCache::new(), SharedParams::new());
        load(&blob, &c2, &s).expect("a payload from another build still loads");
        assert_eq!(s.get(id_for_name("op-0-ratio").unwrap()), 3.5);
        let level = id_for_name("op-0-level").unwrap();
        assert_eq!(
            s.get(level),
            desc(level).unwrap().default,
            "a field the file never mentions must take its descriptor default"
        );
    }

    // ── v1 ──────────────────────────────────────────────────────────────────

    /// An old project must not fail to open. The params restore as they always
    /// did, and the patch resolves to the factory entry the stored index names
    /// — which is what the blob meant, since a patch of that era could not be
    /// anything else.
    #[test]
    fn a_v1_blob_still_loads_and_resolves_its_factory_patch() {
        let a = filled(); // patch 3, saws
        let blob = blob_with(&a, 1, TOTAL_PARAMS, None);
        let (b, s) = (ParamCache::new(), SharedParams::new());
        load(&blob, &b, &s).expect("a v1 blob must still open");

        for id in 0..TOTAL_PARAMS {
            assert_eq!(a.get(id), b.get(id), "param {id}");
        }
        let want = patch(3); // saws, which is what index 3 named then and now
        for id in patch_ids() {
            assert_eq!(
                s.get(id).to_bits(),
                vxn4_engine::params::value_of(&want, id).unwrap().to_bits(),
                "{}",
                desc(id).unwrap().name
            );
        }
        assert_eq!(s.matrix_snapshot(), want.matrix);
        // And the store has recorded the selection, so the next save writes this
        // patch rather than re-basing on it a second time.
        assert_eq!(save(&b, &s), save(&b, &s));
    }

    /// A v1 blob whose body the store has already drifted from must still land
    /// on the factory entry: a v1 restore is a complete description, not a
    /// partial one to be merged with whatever was loaded before.
    #[test]
    fn a_v1_blob_replaces_a_body_the_store_already_had() {
        let c = filled();
        let s = store_for(&c);
        let ratio = id_for_name("op-0-ratio").unwrap();
        s.set(ratio, 3.5);

        load(&blob_with(&c, 1, TOTAL_PARAMS, None), &c, &s).expect("load");
        assert_eq!(
            s.get(ratio),
            vxn4_engine::params::value_of(&patch(3), ratio).unwrap(),
            "the edit outlived a v1 restore"
        );
    }

    // ── refusal ─────────────────────────────────────────────────────────────

    #[test]
    fn rubbish_is_rejected_rather_than_half_loaded() {
        let good = filled();
        let blob = save(&good, &store_for(&good)).expect("save");

        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", Vec::new()),
            ("bad magic", b"NOPE\x02\x00\x00\x00".to_vec()),
            (
                "future version",
                blob_with(&good, VERSION + 1, TOTAL_PARAMS, Some("x")),
            ),
            ("truncated header", blob[..6].to_vec()),
            ("truncated values", blob[..10].to_vec()),
            ("no payload length", blob[..8 + TOTAL_PARAMS * 4].to_vec()),
            ("truncated payload", blob[..blob.len() - 20].to_vec()),
            (
                "payload is not a preset",
                blob_with(&good, 2, TOTAL_PARAMS, Some("not a preset ====")),
            ),
            (
                "payload is a future schema",
                blob_with(
                    &good,
                    2,
                    TOTAL_PARAMS,
                    Some("schema = 99\n[meta]\nname = \"X\"\n"),
                ),
            ),
        ];

        for (what, bad) in cases {
            let (c, s) = (ParamCache::new(), SharedParams::new());
            assert!(load(&bad, &c, &s).is_err(), "{what} should be rejected");
            // A rejected load must not have moved either side.
            for id in 0..TOTAL_PARAMS {
                assert_eq!(c.get(id), default_value(id), "{what}: param {id} moved");
            }
            assert_same_patch(&s, &SharedParams::new(), what);
            assert_eq!(s.topology_backlog(), 0, "{what}: a snapshot was queued");
        }
    }

    /// Non-UTF-8 where the payload should be. Its own case because the length is
    /// honest and only the bytes are wrong, which the truncation checks miss.
    #[test]
    fn a_payload_that_is_not_text_is_rejected() {
        let c = filled();
        let mut blob = save(&c, &store_for(&c)).expect("save");
        let last = blob.len() - 1;
        blob[last] = 0xff;
        let (c2, s) = (ParamCache::new(), SharedParams::new());
        assert!(load(&blob, &c2, &s).is_err());
    }

    /// A blob from a build with a shorter table loads, leaving the params it
    /// never knew about at their defaults.
    #[test]
    fn a_shorter_blob_loads_and_leaves_the_rest_alone() {
        let mut b = Vec::new();
        b.extend_from_slice(&MAGIC);
        b.extend_from_slice(&VERSION.to_le_bytes());
        b.extend_from_slice(&3u16.to_le_bytes());
        for v in [4.0f32, 1.0, 0.5] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let text = "schema = 1\n[meta]\nname = \"X\"\n";
        b.extend_from_slice(&(text.len() as u32).to_le_bytes());
        b.extend_from_slice(text.as_bytes());

        let (c, s) = (ParamCache::new(), SharedParams::new());
        load(&b, &c, &s).expect("short blob should load");
        assert_eq!(c.get(0), 4.0);
        assert_eq!(c.get(1), 1.0);
        assert_eq!(c.get(2), 0.5);
        for m in 0..vxn4_engine::N_MACROS {
            assert_eq!(c.get(N_FIXED + m), 0.0, "macro {m} should be untouched");
        }
    }

    /// A blob from a build with a longer table loads, reading past the tail
    /// rather than desynchronising on it — which now also means finding the
    /// payload length where it really is.
    #[test]
    fn a_longer_blob_loads_and_ignores_the_tail() {
        let a = filled();
        let text = "schema = 1\n[meta]\nname = \"X\"\n[params]\nop-0-ratio = 3.5\n";
        let b = blob_with(&a, VERSION, TOTAL_PARAMS + 4, Some(text));

        let (c, s) = (ParamCache::new(), SharedParams::new());
        load(&b, &c, &s).expect("long blob should load");
        assert_eq!(c.get(0), 3.0);
        assert_eq!(c.get(TOTAL_PARAMS - 1), a.get(TOTAL_PARAMS - 1));
        assert_eq!(
            s.get(id_for_name("op-0-ratio").unwrap()),
            3.5,
            "the payload was not found past the value tail"
        );
    }

    /// A corrupted value cannot become an out-of-range patch index.
    #[test]
    fn a_corrupt_value_is_clamped_not_trusted() {
        let c = ParamCache::new();
        let mut b = save(&c, &store_for(&c)).expect("save");
        b[8..12].copy_from_slice(&1e9f32.to_le_bytes());
        let (c2, s) = (ParamCache::new(), SharedParams::new());
        load(&b, &c2, &s).expect("load");
        assert_eq!(c2.get(0), (vxn4_engine::N_PATCHES - 1) as f32);
        // ...and the store agrees about which patch that was.
        assert_eq!(
            params::patch_from(s.get(selection_id())),
            vxn4_engine::N_PATCHES - 1
        );
    }

    // ── the ownership rule ──────────────────────────────────────────────────

    /// A selection the store has not seen is adopted; one it has is left alone,
    /// body and all. Those two halves are the whole rule.
    #[test]
    fn the_store_rebases_on_a_new_selection_and_only_on_that() {
        let c = ParamCache::new();
        let s = SharedParams::new();
        let ratio = id_for_name("op-0-ratio").unwrap();

        // A fresh pair already agrees on patch 0, so there is nothing to adopt.
        assert_eq!(rebase_selection(&c, &s), 0);
        assert_eq!(s.topology_backlog(), 0, "a matching selection re-based");

        s.set(ratio, 3.5);
        assert_eq!(rebase_selection(&c, &s), 0);
        assert_eq!(s.get(ratio), 3.5, "an edited body was re-based away");

        // Moving the selection does adopt, edits included.
        c.set(0, 4.0);
        assert_eq!(rebase_selection(&c, &s), 4);
        assert_eq!(
            s.get(ratio),
            vxn4_engine::params::value_of(&patch(4), ratio).unwrap()
        );
    }
}
