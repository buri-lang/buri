//! The JavaScript backend's crossing table: what a Buri value is on the other
//! side of a repository platform's `js` file.
//!
//! Two places cross. An entry with a `js` file is handed to that file as a
//! JavaScript function, so its parameters arrive from JavaScript and its answer
//! leaves for it. A method a platform's `platform.buri` declares without a body
//! is the file's to implement, so the other way round.
//!
//! | Buri                    | JavaScript                     |
//! | ----------------------- | ------------------------------ |
//! | `Str`, `Bool`, `F64`    | `string`, `boolean`, `number`  |
//! | `Int`, other integers   | `bigint`                       |
//! | `[U8]`                  | `Uint8Array`                   |
//! | `[T]`, a tuple          | `Array`                        |
//! | `Option<T>`, `()`       | the value, or `undefined`      |
//! | `Result<T, Str>`        | the value, or a thrown `Error` |
//! | a struct                | a plain object of its fields   |
//! | `Request`, `Response`   | the Fetch standard's           |
//!
//! `Option<Option<T>>` and `Option<()>` don't cross: `None` and `Some` would
//! both be `undefined`.
//!
//! The checker refuses anything else as `type-not-crossable`, and the backend
//! reads the same answer to write the conversion, so the two cannot disagree
//! about what crosses.

use crate::compiler::semantics::types::{substitute, Prim, Tables, Ty, TyKind, TyConId, TyDef};

/// How one value crosses.
#[derive(Clone, Debug, PartialEq)]
pub enum Crossing {
    /// The same value on both sides: `Str`, `Bool`, the floats, and the two
    /// integers that are already a `bigint` (`I64`, `U64`).
    Same,
    /// An integer held as a `number`, which is a `bigint` on the other side.
    Widened,
    /// `[U8]`, an array of numbers here and a `Uint8Array` there.
    Bytes,
    List(Box<Crossing>),
    Tuple(Vec<Crossing>),
    /// `Option<T>`: the value, or `undefined`.
    Optional(Box<Crossing>),
    /// `()`, which is `undefined` on the other side.
    Unit,
    /// `Result<T, Str>`, only as a whole answer: the value, or a thrown
    /// `Error` whose message is the `Str`.
    Result(Box<Crossing>),
    /// A struct: a plain object with one property per field, in order.
    Record(Vec<(String, Crossing)>),
    /// `platform/effect`'s `Request`, the Fetch standard's on the other side.
    Request,
    /// `platform/effect`'s `Response`, the same.
    Response,
}

impl Crossing {
    /// Whether the value is the same on both sides all the way down, so no
    /// conversion is written.
    pub fn is_same(&self) -> bool {
        match self {
            Crossing::Same => true,
            // An absent value may arrive as `null`, so an `Option` always converts.
            Crossing::List(e) => e.is_same(),
            Crossing::Tuple(items) => items.iter().all(Crossing::is_same),
            _ => false,
        }
    }

    /// Whether converting a value from JavaScript waits: a `Request` or a
    /// `Response` is read as a stream.
    pub fn waits(&self) -> bool {
        match self {
            Crossing::Request | Crossing::Response => true,
            Crossing::List(e) | Crossing::Optional(e) | Crossing::Result(e) => e.waits(),
            Crossing::Tuple(items) => items.iter().any(Crossing::waits),
            Crossing::Record(fields) => fields.iter().any(|(_, c)| c.waits()),
            _ => false,
        }
    }
}

/// The two nominal types the table names by identity.
#[derive(Clone, Copy, Debug, Default)]
pub struct Known {
    pub request: Option<TyConId>,
    pub response: Option<TyConId>,
}

/// How `ty` crosses, or the part of it that cannot.
///
/// `answer` is whether `ty` is a whole answer — an entry's or a method's
/// result — which is the one place `Result<T, Str>` crosses.
pub fn classify(tables: &Tables, known: Known, ty: &Ty, answer: bool) -> Result<Crossing, Ty> {
    classify_in(tables, known, ty, answer, &mut Vec::new())
}

fn classify_in(
    tables: &Tables,
    known: Known,
    ty: &Ty,
    answer: bool,
    open: &mut Vec<TyConId>,
) -> Result<Crossing, Ty> {
    match ty.kind() {
        TyKind::Unit => Ok(Crossing::Unit),
        // Already reported where it was written.
        TyKind::Error => Ok(Crossing::Same),
        TyKind::Tuple(items) => items
            .iter()
            .map(|t| classify_in(tables, known, t, false, open))
            .collect::<Result<Vec<_>, _>>()
            .map(Crossing::Tuple),
        TyKind::Array(elem) => {
            if tables.as_prim(elem) == Some(Prim::U8) {
                return Ok(Crossing::Bytes);
            }
            Ok(Crossing::List(Box::new(classify_in(tables, known, elem, false, open)?)))
        }
        TyKind::Con(id, args) => {
            let con = *id;
            if Some(con) == known.request && args.is_empty() {
                return Ok(Crossing::Request);
            }
            if Some(con) == known.response && args.is_empty() {
                return Ok(Crossing::Response);
            }
            if let Some(p) = tables.as_prim(ty) {
                return match p {
                    Prim::Bool | Prim::Str | Prim::F32 | Prim::F64 | Prim::I64 | Prim::U64 => {
                        Ok(Crossing::Same)
                    }
                    Prim::I8
                    | Prim::I16
                    | Prim::I32
                    | Prim::I128
                    | Prim::U8
                    | Prim::U16
                    | Prim::U32
                    | Prim::U128 => Ok(Crossing::Widened),
                    Prim::Char | Prim::Template => Err(ty.clone()),
                };
            }
            if let Some(payload) = tables.option_payload(ty) {
                // `Some(None)` would be `undefined` too, and so would `Some(())`.
                if tables.is_option_ty(payload) || *payload == Ty::UNIT {
                    return Err(ty.clone());
                }
                let inner = classify_in(tables, known, payload, false, open)?;
                return Ok(Crossing::Optional(Box::new(inner)));
            }
            if is_result(tables, con) {
                let str_ty = tables.prim(Prim::Str);
                return match args {
                    [ok, err] if answer && *err == str_ty => Ok(Crossing::Result(Box::new(
                        classify_in(tables, known, ok, false, open)?,
                    ))),
                    _ => Err(ty.clone()),
                };
            }
            let tycon = tables.tycon(con);
            match &tycon.def {
                TyDef::Struct { fields, .. } => {
                    // A type that holds itself would cross for ever.
                    if open.contains(&con) {
                        return Err(ty.clone());
                    }
                    open.push(con);
                    let mut out = Vec::new();
                    for f in fields {
                        let field_ty = substitute(&f.ty, args, None);
                        match classify_in(tables, known, &field_ty, false, open) {
                            Ok(c) => out.push((f.name.clone(), c)),
                            Err(bad) => {
                                open.pop();
                                return Err(bad);
                            }
                        }
                    }
                    open.pop();
                    Ok(Crossing::Record(out))
                }
                _ => Err(ty.clone()),
            }
        }
        TyKind::Var(_) | TyKind::Param(_) | TyKind::Fn(..) | TyKind::Ctx(_) | TyKind::SelfTy => Err(ty.clone()),
    }
}

/// Whether a type constructor is the prelude's `Result`, by shape as well as
/// by name, as [`Tables::is_option`] tells `Option`.
pub fn is_result(tables: &Tables, con: TyConId) -> bool {
    let t = tables.tycon(con);
    t.name == "Result"
        && t.generics.len() == 2
        && matches!(&t.def, TyDef::Enum { variants }
            if matches!(variants.as_slice(), [ok, err]
                if ok.name == "Ok" && ok.fields.len() == 1 && err.name == "Err" && err.fields.len() == 1))
}
