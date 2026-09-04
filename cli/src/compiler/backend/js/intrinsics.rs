//! The bodies of the operations the standard library declares without one.
//!
//! Most are a call into the runtime: the key `list.map` becomes `$list_map`.
//! The exceptions are the numeric methods of `core/num`, which are mechanical
//! enough to emit inline — there is one conversion per source-and-target pair
//! (SPEC 6.2.1), and generating `Number(x)` beats calling a runtime function
//! that does the same.

use crate::compiler::backend::js::generate::{as_int_n, as_uint_n, Gen};
use crate::compiler::backend::js::javascript::{BinOp, Expr};
use crate::compiler::semantics::builtins::conversion_is_exact;
use crate::compiler::semantics::types::Prim;
use crate::compiler::middle::monomorphize::Func;

impl<'a> Gen<'a> {
    pub(crate) fn intrinsic(&mut self, key: &str, args: &[Expr], f: &Func) -> Option<Expr> {
        let parts: Vec<&str> = key.split('.').collect();
        match parts.as_slice() {
            ["num", ty, name] => return self.numeric(ty, name, args),
            ["num", name] => return self.numeric_free(name, f),
            _ => {}
        }
        // `json.decode` is asked for a type rather than handed one, so what it
        // compiles to is the descriptor walk over `T` and the document. The
        // context is the allocator, which the JavaScript runtime does not
        // need, so it is dropped here rather than passed and ignored.
        if key == "json.decode" {
            let d = f.desc?;
            return Some(Expr::call(
                Expr::ident("$json_decode"),
                vec![
                    args.get(1)?.clone(),
                    Expr::ident(crate::compiler::backend::js::generate::descriptor_name(d)),
                ],
            ));
        }
        // Everything else is a runtime function of the same name.
        let name = format!("${}", key.replace('.', "_"));
        if self.runtime_has(&name) {
            let mut all = args.to_vec();
            if let Some(d) = f.desc {
                all.push(Expr::ident(crate::compiler::backend::js::generate::descriptor_name(d)));
            }
            return Some(Expr::call(Expr::ident(name), all));
        }
        // The four structural operations are defined for every primitive, and
        // the runtime implements them once rather than per type.
        match parts.last().copied() {
            Some("eq") if args.len() == 2 => {
                let (x, y) = (args.first()?, args.get(1)?);
                Some(Expr::bin(BinOp::StrictEq, x.clone(), y.clone()))
            }
            Some("compare") if args.len() == 2 => Some(
                compare_inline(args.first()?, args.get(1)?)
                    .unwrap_or_else(|| Expr::call(Expr::ident("$cmp"), args.to_vec())),
            ),
            Some("hash") => Some(Expr::call(Expr::ident("$hash"), vec![args.first()?.clone()])),
            Some("show") => Some(Expr::call(Expr::ident("$str"), vec![args.first()?.clone()])),
            // `Str` and `Char` are both JavaScript strings, and a `Char` is a
            // one-character one, so both are a JSON string.
            Some("toJson") => {
                let helper = if parts.first() == Some(&"bool") { "$json_bool" } else { "$json_str" };
                Some(Expr::call(Expr::ident(helper), vec![args.first()?.clone()]))
            }
            _ => None,
        }
    }

    /// `Bounded`'s methods take no `self`, so `num.minValue<U8>()` reaches
    /// them through the return type.
    fn numeric_free(&mut self, name: &str, f: &Func) -> Option<Expr> {
        let p = self.prim_of(&f.ret)?;
        // Every built-in integer type satisfies `Bounded`, `Checked` and
        // `Wrapping`; the float types satisfy `Bounded` only (SPEC 6.2.2).
        if p.is_float() {
            return Some(match (name, p) {
                ("minValue", Prim::F32) => Expr::Num(-(f32::MAX as f64)),
                ("maxValue", Prim::F32) => Expr::Num(f32::MAX as f64),
                ("minValue", _) => Expr::Num(f64::MIN),
                ("maxValue", _) => Expr::Num(f64::MAX),
                _ => return None,
            });
        }
        let (lo, hi) = p.int_range()?;
        match name {
            "minValue" => Some(int_const(p, lo)),
            "maxValue" => Some(upper_const(p, hi)),
            _ => None,
        }
    }

    fn numeric(&mut self, ty: &str, name: &str, args: &[Expr]) -> Option<Expr> {
        let from = Prim::all().iter().copied().find(|p| p.name() == ty)?;
        let a = args.first().cloned();

        // --- Conversions ---------------------------------------------------
        if name == "toChar" {
            return Some(Expr::call(Expr::ident("$toChar"), vec![a?]));
        }
        if let Some(target) = name.strip_prefix("wrapTo") {
            let to = Prim::all().iter().copied().find(|p| p.name() == target)?;
            return Some(self.wrap_conversion(a?, from, to));
        }
        if let Some(target) = name.strip_prefix("to") {
            if let Some(to) = Prim::all().iter().copied().find(|p| p.name() == target) {
                return Some(self.conversion(a?, from, to));
            }
        }

        // Every one of these is declared with two parameters, so a call with
        // fewer is a missing intrinsic rather than a crash.
        let two = || Some((args.first()?.clone(), args.get(1)?.clone()));
        match name {
            "abs" => {
                // `abs` of a signed minimum overflows, and overflow is
                // undefined, so there is nothing to check.
                let v = a?;
                Some(if from.is_bigint() {
                    Expr::call(Expr::ident("$absBig"), vec![v])
                } else {
                    Expr::call(Expr::member(Expr::ident("Math"), "abs"), vec![v])
                })
            }
            "signum" => {
                let v = a?;
                // The answer is the argument's own type, so its three values
                // are written in the argument's own representation.
                let zero = int_const(from, 0);
                let one = int_const(from, 1);
                let minus_one = int_const(from, -1);
                Some(Expr::cond(
                    Expr::bin(BinOp::Lt, v.clone(), zero.clone()),
                    minus_one,
                    Expr::cond(Expr::bin(BinOp::Gt, v, zero.clone()), one, zero),
                ))
            }
            "eq" => {
                let (x, y) = two()?;
                Some(if from.is_float() {
                    crate::compiler::backend::js::generate::float_eq(x, y)
                } else {
                    Expr::bin(BinOp::StrictEq, x, y)
                })
            }
            "compare" => {
                let (x, y) = two()?;
                Some(
                    compare_inline(&x, &y)
                        .unwrap_or_else(|| Expr::call(Expr::ident("$cmp"), vec![x, y])),
                )
            }
            "hash" => Some(Expr::call(Expr::ident("$hash"), vec![a?])),
            // JSON has one number type and it is a double, so a `BigInt` is
            // narrowed on the way in — a document cannot hold the value a
            // `BigInt` can, and `JSON.stringify` refuses one outright.
            "toJson" => {
                let v = a?;
                let num = if from.is_bigint() {
                    Expr::call(Expr::ident("Number"), vec![v])
                } else {
                    v
                };
                Some(Expr::call(Expr::ident("$json_num"), vec![num]))
            }
            "show" => {
                let v = a?;
                Some(if from.is_float() {
                    Expr::call(Expr::ident("$f64"), vec![v])
                } else if from.is_integer() {
                    // An integer is a `number` or a `BigInt`, and `String`
                    // renders both in decimal with no suffix; `$str` would
                    // render the first as a float.
                    Expr::call(Expr::ident("String"), vec![v])
                } else {
                    Expr::call(Expr::ident("$str"), vec![v])
                })
            }
            "add" | "sub" | "mul" | "div" | "rem" | "neg" => {
                let op = match name {
                    "add" => crate::compiler::semantics::typed::PrimOp::Add,
                    "sub" => crate::compiler::semantics::typed::PrimOp::Sub,
                    "mul" => crate::compiler::semantics::typed::PrimOp::Mul,
                    "div" => crate::compiler::semantics::typed::PrimOp::Div,
                    "rem" => crate::compiler::semantics::typed::PrimOp::Rem,
                    _ => crate::compiler::semantics::typed::PrimOp::Neg,
                };
                Some(self.prim_op_pub(op, from, args.to_vec()))
            }
            // The default `+` leaves overflow undefined; these are the
            // alternatives, spelled out where they are used. The bound is the
            // type's own range on every backend, so a `.Some` is always a value
            // the answer really is and `.None` always means overflow.
            "checkedAdd" | "checkedSub" | "checkedMul" | "checkedDiv" => {
                let (x, y) = two()?;
                let (lo, hi) = from.int_range()?;
                let bound = if from.is_bigint() { "$checkedInBig" } else { "$checkedIn" };
                let op = match name {
                    "checkedAdd" => BinOp::Add,
                    "checkedSub" => BinOp::Sub,
                    "checkedMul" => BinOp::Mul,
                    _ => BinOp::Div,
                };
                let raw = if op == BinOp::Div {
                    let zero = int_const(from, 0);
                    // A checked division by zero is `.None`, not an abort.
                    let div = if from.is_bigint() {
                        Expr::bin(BinOp::Div, x.clone(), y.clone())
                    } else {
                        Expr::call(
                            Expr::member(Expr::ident("Math"), "trunc"),
                            vec![Expr::bin(BinOp::Div, x.clone(), y.clone())],
                        )
                    };
                    return Some(Expr::cond(
                        Expr::bin(BinOp::StrictEq, y, zero),
                        // `None` is absence itself.
                        Expr::Undefined,
                        Expr::call(
                            Expr::ident(bound),
                            vec![div, int_const(from, lo), upper_const(from, hi)],
                        ),
                    ));
                } else {
                    Expr::bin(op, x, y)
                };
                Some(Expr::call(
                    Expr::ident(bound),
                    vec![raw, int_const(from, lo), upper_const(from, hi)],
                ))
            }
            // `Wrapping` is the surface a program uses when it wants the bit
            // pattern — a checksum, a hash, a wire format — so the answer has
            // to be the low bits of the *exact* result rather than the low bits
            // of a double that was rounded on the way.
            //
            // At a `BigInt` width the operation is already exact and the wrap
            // is one `asIntN`. Below it the operands and the answer are exact
            // doubles but the intermediate need not be — `U32.wrappingMul(
            // 0xffffffff, 0xffffffff)` is 1, its exact product rounds to an
            // even double, and the wrap of that was 0 — so a product that can
            // leave 2^53 is computed in `BigInt` and wrapped there.
            "wrappingAdd" | "wrappingSub" | "wrappingMul" => {
                let (x, y) = two()?;
                let (op, selector) = match name {
                    "wrappingAdd" => (BinOp::Add, 0.0),
                    "wrappingSub" => (BinOp::Sub, 1.0),
                    _ => (BinOp::Mul, 2.0),
                };
                if from.is_bigint() {
                    return Some(self.wrap_bits(Expr::bin(op, x, y), from, from));
                }
                let bits = u64::from(from.bits());
                let widest = if op == BinOp::Mul { bits.saturating_mul(2) } else { bits + 1 };
                if widest <= 53 {
                    return Some(self.wrap_bits(Expr::bin(op, x, y), from, from));
                }
                Some(Expr::call(
                    Expr::ident("$wrapOp"),
                    vec![
                        Expr::Num(selector),
                        x,
                        y,
                        Expr::Num(bits as f64),
                        Expr::Bool(from.is_signed()),
                    ],
                ))
            }
            "saturatingAdd" | "saturatingSub" | "saturatingMul" => {
                let (x, y) = two()?;
                let op = match name {
                    "saturatingAdd" => BinOp::Add,
                    "saturatingSub" => BinOp::Sub,
                    _ => BinOp::Mul,
                };
                let (lo, hi) = from.int_range()?;
                Some(Expr::call(
                    Expr::ident("$sat"),
                    vec![Expr::bin(op, x, y), int_const(from, lo), upper_const(from, hi)],
                ))
            }
            "minValue" => {
                if from.is_float() {
                    return Some(if from == Prim::F32 {
                        Expr::Num(-(f32::MAX as f64))
                    } else {
                        Expr::Num(f64::MIN)
                    });
                }
                let (lo, _) = from.int_range()?;
                Some(int_const(from, lo))
            }
            "maxValue" => {
                if from.is_float() {
                    return Some(if from == Prim::F32 {
                        Expr::Num(f32::MAX as f64)
                    } else {
                        Expr::Num(f64::MAX)
                    });
                }
                let (_, hi) = from.int_range()?;
                Some(upper_const(from, hi))
            }
            _ => None,
        }
    }

    /// `x.toT()`: exact conversions are a representation change and nothing
    /// more; the rest return `Result<T, RangeError>`.
    fn conversion(&mut self, v: Expr, from: Prim, to: Prim) -> Expr {
        if from == to {
            return v;
        }
        if conversion_is_exact(from, to) {
            return represent(v, from, to);
        }
        // A float target has no integer range; `F64 -> F32` fails only when
        // the value does not survive as a finite binary32.
        let Some((lo, hi)) = to.int_range() else {
            return Expr::call(
                Expr::ident("$convF32"),
                vec![v, Expr::Str(to.name().to_string())],
            );
        };
        Expr::call(
            Expr::ident("$convChecked"),
            vec![
                v,
                int_const(to, lo),
                upper_const(to, hi),
                Expr::Str(to.name().to_string()),
                Expr::Bool(from.is_float()),
                Expr::Bool(to.is_bigint()),
            ],
        )
    }

    /// `x.wrapToT()`: modular for integers, rounding for floats.
    fn wrap_conversion(&mut self, v: Expr, from: Prim, to: Prim) -> Expr {
        if to.is_float() {
            // "wraps (integers) or rounds (floats)" — rounding to binary32 is
            // what `Math.fround` is.
            return represent(v, from, to);
        }
        let value = if from.is_float() {
            Expr::call(Expr::member(Expr::ident("Math"), "trunc"), vec![v])
        } else {
            v
        };
        self.wrap_bits(value, from, to)
    }

    /// Taking the low bits of a value. At a `BigInt` target that is the
    /// operator itself; elsewhere the runtime builds one, because a double
    /// cannot hold the intermediate.
    fn wrap_bits(&mut self, value: Expr, from: Prim, to: Prim) -> Expr {
        if to.is_bigint() {
            let wide = if from.is_bigint() {
                value
            } else {
                Expr::call(Expr::ident("BigInt"), vec![value])
            };
            return if to.is_signed() {
                as_int_n(to.bits(), wide)
            } else {
                as_uint_n(to.bits(), wide)
            };
        }
        Expr::call(
            Expr::ident("$wrapTo"),
            vec![value, Expr::Num(f64::from(to.bits())), Expr::Bool(to.is_signed())],
        )
    }

}

/// The same value, written the way the target type is written. A widening is
/// exact and changes nothing else; what changes is whether the value is a
/// `number` or a `BigInt`, and only one of the two directions of that is ever
/// lossy — `Number(bigint)` rounds, which is what `toF64` promises (SPEC
/// 6.2.1).
fn represent(v: Expr, from: Prim, to: Prim) -> Expr {
    if to == Prim::F32 {
        let widened = represent(v, from, Prim::F64);
        return Expr::call(Expr::member(Expr::ident("Math"), "fround"), vec![widened]);
    }
    match (from.is_bigint(), to.is_bigint()) {
        (false, true) => Expr::call(Expr::ident("BigInt"), vec![v]),
        (true, false) => Expr::call(Expr::ident("Number"), vec![v]),
        _ => v,
    }
}

/// A constant in the representation its type has: a `number` at the narrow
/// widths, a `BigInt` at the wide ones. Every bound this file writes down goes
/// through here, so a comparison never mixes the two by accident.
fn int_const(p: Prim, v: i128) -> Expr {
    if p.is_bigint() { Expr::BigInt(v.to_string()) } else { Expr::Num(v as f64) }
}

/// The upper bound, which for `U128` does not fit in an `i128` — casting it
/// there turns `maxValue` and every `Checked`/`Saturating` bound into `-1`.
fn upper_const(p: Prim, v: u128) -> Expr {
    if p.is_bigint() { Expr::BigInt(v.to_string()) } else { Expr::Num(v as f64) }
}

/// `compare` at a primitive: three comparisons instead of a call into a walker
/// that begins by asking whether either side is `None` and whether either is
/// an array — neither of which a primitive ever is.
///
/// `Order`'s variants carry nothing, so it is a bare number: 0 Less, 1 Equal,
/// 2 Greater, which is what `$cmp` answers too. The comparisons agree with it
/// at the awkward values by construction: `NaN` is unordered, so both tests
/// fail and the answer is Equal, and `-0.0 < 0.0` is false both ways.
///
/// Only where both operands are free to duplicate. The ternary reads each of
/// them twice, and binding them first would cost what the call cost.
fn compare_inline(a: &Expr, b: &Expr) -> Option<Expr> {
    if !a.is_pure_literal() || !b.is_pure_literal() {
        return None;
    }
    // Never for text. A `Str` and a `Char` are both JavaScript strings, and `<`
    // on one is UTF-16 code-unit order rather than the scalar order the
    // language specifies — so `'\u{1F600}'.compare('\u{E000}')` folded here
    // would answer `Less` where `$str_compare` answers `Greater`. `$cmp` routes
    // text through `$str_compare`; this shortcut is for the numbers and the
    // booleans, where `<` is already the answer.
    if matches!(a, Expr::Str(_)) || matches!(b, Expr::Str(_)) {
        return None;
    }
    Some(Expr::cond(
        Expr::bin(BinOp::Lt, a.clone(), b.clone()),
        Expr::Num(0.0),
        Expr::cond(
            Expr::bin(BinOp::Gt, a.clone(), b.clone()),
            Expr::Num(2.0),
            Expr::Num(1.0),
        ),
    ))
}
