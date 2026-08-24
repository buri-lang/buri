//! The `buri_rt_*` boundary for this backend: the shared table, and how an
//! `Option` reaches its slot.
//!
//! `cli/runtime/lib.rs`'s module comment is the contract, and
//! `backend/runtime_table.rs` is the transcription of it that generated code
//! is emitted against — shared with the Cranelift backend, which emits the
//! same call from the same rows. That file's header carries the four shapes
//! and the emission rule.
//!
//! What is different here is only where the arguments *are*: this backend's
//! calling convention is frame-threaded, so a leaf is a byte offset rather
//! than a register, and `rtcall.rs` copies them into the scratch area that one
//! `crt` stencil then reads (`sources.rs::runtime_calls`).
//!
//! What is in this file is the one thing the shared table cannot hold:
//! [`OptRepr`], which is `middle::layout`'s answer about an `Option` flattened
//! into literals, because a store here is a stencil and a stencil takes
//! literals.

use crate::compiler::middle::layout::{EnumRepr, Layout, Repr};

/// The table, and the shapes it is written in.
///
/// `backend/runtime_table.rs` holds them, because Cranelift emits the same
/// call from the same rows. They are re-exported here so that `runtime::` is
/// still where this backend's emitter looks.
pub use crate::compiler::backend::runtime_table::{entry, Entry, Extra, Ret, BURI_OK, ENTRIES};

/// How an `Option<T>` is written, flattened out of `middle::layout` so that the
/// emitter never learns which niche the layout chose.
///
/// `cranelift/emit.rs::option_call` asks the layout the same four questions
/// inline; this backend needs them as data because the store is a stencil and
/// a stencil takes literals.
#[derive(Clone, Copy, Debug)]
pub struct OptRepr {
    /// Byte offset of the discriminant, and its width; for a niche, the offset
    /// of the pointer that is null when the value is `.None`, at width eight.
    pub tag: (u32, u32),
    /// Whether `.None` is a null pointer rather than a stored tag.
    pub niche: bool,
    /// Byte offset of `.Some`'s payload.
    pub payload: u32,
    /// The discriminants themselves.
    pub some: u64,
    pub none: u64,
}

impl OptRepr {
    /// Reads the four facts off a destination's layout, or answers `None` when
    /// the destination is not an enum with an empty variant — which is what an
    /// `Option` is, structurally, and the only thing this may be asked about.
    pub fn of(l: &Layout) -> Option<OptRepr> {
        let Repr::Enum { repr, variants } = &l.repr else { return None };
        // `Option` declares `Some` first, so `Some` is variant 0 — but read it
        // off the layout rather than assuming: the empty variant is the one
        // with no fields.
        let none = variants.iter().position(|v| v.is_empty())? as u64;
        let some = u64::from(none == 0);
        let payload = variants
            .get(some as usize)
            .and_then(|v| v.first())
            .copied()
            .unwrap_or(0);
        Some(match repr {
            EnumRepr::Bare { tag } => {
                OptRepr { tag: (0, tag.size()), niche: false, payload: 0, some, none }
            }
            EnumRepr::Tagged { tag, .. } => {
                OptRepr { tag: (0, tag.size()), niche: false, payload, some, none }
            }
            EnumRepr::Niche { null_at } => {
                OptRepr { tag: (*null_at, 8), niche: true, payload, some, none }
            }
        })
    }
}
