//! The bodies of the operations the standard library declares without one.
//!
//! Most are a call into the runtime: the key `list.map` becomes `$list_map`.
//! The exceptions are the numeric methods of `core/num`, which are mechanical
//! enough to emit inline — there is one conversion per source-and-target pair
//! (SPEC 6.2.1), and generating `Number(x)` beats calling a runtime function
//! that does the same.

use crate::builtins::conversion_is_exact;
use crate::codegen::Gen;
use crate::js::{BinOp, Expr, UnOp};
use crate::mono::Func;
use crate::types::Prim;

impl<'a> Gen<'a> {
    pub(crate) fn intrinsic(&mut self, key: &str, args: &[Expr], f: &Func) -> Option<Expr> {
        let parts: Vec<&str> = key.split('.').collect();
        if parts.len() == 3 && parts[0] == "num" {
            return self.numeric(parts[1], parts[2], args, f);
        }
        if parts.len() == 2 && parts[0] == "num" {
            return self.numeric_free(parts[1], args, f);
        }
        // Everything else is a runtime function of the same name.
        let name = format!("${}", key.replace('.', "_"));
        if self.runtime_has(&name) {
            let mut all = args.to_vec();
            if let Some(d) = f.desc {
                all.push(Expr::ident(crate::codegen::descriptor_name(d)));
            }
            return Some(Expr::call(Expr::ident(name), all));
        }
        // The four structural operations are defined for every primitive, and
        // the runtime implements them once rather than per type.
        match parts.last().copied() {
            Some("eq") if args.len() == 2 => {
                Some(Expr::bin(BinOp::StrictEq, args[0].clone(), args[1].clone()))
            }
            Some("compare") if args.len() == 2 => {
                Some(Expr::call(Expr::ident("$cmp"), args.to_vec()))
            }
            Some("hash") if !args.is_empty() => {
                Some(Expr::call(Expr::ident("$hash"), vec![args[0].clone()]))
            }
            Some("show") if !args.is_empty() => {
                Some(Expr::call(Expr::ident("$str"), vec![args[0].clone()]))
            }
            _ => None,
        }
    }

    /// `Bounded`'s methods take no `self`, so `num.minValue::<U8>()` reaches
    /// them through the return type.
    fn numeric_free(&mut self, name: &str, _args: &[Expr], f: &Func) -> Option<Expr> {
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
            "minValue" => Some(self.int_const(lo, p)),
            "maxValue" => Some(self.upper_const(hi, p)),
            _ => None,
        }
    }

    fn numeric(&mut self, ty: &str, name: &str, args: &[Expr], _f: &Func) -> Option<Expr> {
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

        let two = || (args[0].clone(), args[1].clone());
        match name {
            "abs" => {
                // `abs` of a signed minimum overflows, and overflow is
                // undefined, so there is nothing to check.
                let v = a?;
                Some(Expr::call(Expr::member(Expr::ident("Math"), "abs"), vec![v]))
            }
            "signum" => {
                let v = a?;
                let zero = self.zero(from);
                let one = self.one(from);
                let minus_one = Expr::un(UnOp::Neg, self.one(from));
                Some(Expr::cond(
                    Expr::bin(BinOp::Lt, v.clone(), zero.clone()),
                    minus_one,
                    Expr::cond(Expr::bin(BinOp::Gt, v, zero), one, self.zero(from)),
                ))
            }
            "eq" => {
                let (x, y) = two();
                Some(Expr::bin(BinOp::StrictEq, x, y))
            }
            "compare" => {
                let (x, y) = two();
                Some(Expr::call(Expr::ident("$cmp"), vec![x, y]))
            }
            "hash" => Some(Expr::call(Expr::ident("$hash"), vec![a?])),
            "show" => {
                let v = a?;
                Some(if from.is_float() {
                    Expr::call(Expr::ident("$f64"), vec![v])
                } else if from.is_integer() && !from.is_bigint() {
                    // A narrow integer is a JS number; `$str` would render it
                    // as a float.
                    Expr::call(Expr::ident("String"), vec![v])
                } else {
                    Expr::call(Expr::ident("$str"), vec![v])
                })
            }
            "add" | "sub" | "mul" | "div" | "rem" | "neg" => {
                let op = match name {
                    "add" => crate::hir::PrimOp::Add,
                    "sub" => crate::hir::PrimOp::Sub,
                    "mul" => crate::hir::PrimOp::Mul,
                    "div" => crate::hir::PrimOp::Div,
                    "rem" => crate::hir::PrimOp::Rem,
                    _ => crate::hir::PrimOp::Neg,
                };
                Some(self.prim_op_pub(op, Some(from), args.to_vec()))
            }
            // The default `+` leaves overflow undefined; these are the
            // alternatives, spelled out where they are used. The bounds are
            // the exactly-representable ones, so a `.Some` is always a value
            // the answer really is.
            "checkedAdd" | "checkedSub" | "checkedMul" | "checkedDiv" => {
                let (x, y) = two();
                let (lo, hi) = from.exact_int_range()?;
                let op = match name {
                    "checkedAdd" => BinOp::Add,
                    "checkedSub" => BinOp::Sub,
                    "checkedMul" => BinOp::Mul,
                    _ => BinOp::Div,
                };
                let raw = if op == BinOp::Div {
                    let zero = self.zero(from);
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
                            Expr::ident("$checkedIn"),
                            vec![div, self.int_const(lo, from), self.upper_const(hi, from)],
                        ),
                    ));
                } else {
                    Expr::bin(op, x, y)
                };
                Some(Expr::call(
                    Expr::ident("$checkedIn"),
                    vec![raw, self.int_const(lo, from), self.upper_const(hi, from)],
                ))
            }
            "wrappingAdd" | "wrappingSub" | "wrappingMul" => {
                let (x, y) = two();
                let op = match name {
                    "wrappingAdd" => BinOp::Add,
                    "wrappingSub" => BinOp::Sub,
                    _ => BinOp::Mul,
                };
                Some(self.wrap_value(Expr::bin(op, x, y), from))
            }
            "saturatingAdd" | "saturatingSub" | "saturatingMul" => {
                let (x, y) = two();
                let op = match name {
                    "saturatingAdd" => BinOp::Add,
                    "saturatingSub" => BinOp::Sub,
                    _ => BinOp::Mul,
                };
                let (lo, hi) = from.int_range()?;
                Some(Expr::call(
                    Expr::ident("$sat"),
                    vec![
                        Expr::bin(op, x, y),
                        self.int_const(lo, from),
                        self.upper_const(hi, from),
                    ],
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
                Some(self.int_const(lo, from))
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
                Some(self.upper_const(hi, from))
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
        let exact = conversion_is_exact(from, to);
        let mut widened = self.represent(v.clone(), from, to);
        if to == Prim::F32 {
            widened = Expr::call(Expr::member(Expr::ident("Math"), "fround"), vec![widened]);
        }
        if exact {
            return widened;
        }
        // A float target has no integer range; `F64 -> F32` fails only when
        // the value does not survive as a finite binary32.
        let Some((lo, hi)) = to.exact_int_range() else {
            return Expr::call(
                Expr::ident("$convF32"),
                vec![v, Expr::Str(to.name().to_string())],
            );
        };
        Expr::call(
            Expr::ident("$convChecked"),
            vec![
                v,
                self.int_const(lo, to),
                self.upper_const(hi, to),
                Expr::Str(to.name().to_string()),
                Expr::Bool(from.is_float()),
            ],
        )
    }

    /// `x.wrapToT()`: modular for integers, rounding for floats.
    fn wrap_conversion(&mut self, v: Expr, from: Prim, to: Prim) -> Expr {
        if to.is_float() {
            let widened = self.represent(v, from, to);
            // "wraps (integers) or rounds (floats)" — rounding to binary32 is
            // what `Math.fround` is.
            return if to == Prim::F32 {
                Expr::call(Expr::member(Expr::ident("Math"), "fround"), vec![widened])
            } else {
                widened
            };
        }
        let value = if from.is_float() {
            Expr::call(Expr::member(Expr::ident("Math"), "trunc"), vec![v])
        } else {
            v
        };
        self.wrap_bits(value, to)
    }

    /// Taking the low bits of a value is the one place a `BigInt` is still the
    /// right tool: it is exact where a double is not. The result comes back as
    /// a `number`.
    fn wrap_bits(&mut self, value: Expr, to: Prim) -> Expr {
        Expr::call(
            Expr::ident("$wrapTo"),
            vec![
                value,
                Expr::Num(to.bits() as f64),
                Expr::Bool(to.is_signed()),
            ],
        )
    }

    /// Wraps an already-computed value back into its own type's range.
    fn wrap_value(&mut self, v: Expr, p: Prim) -> Expr {
        self.wrap_bits(v, p)
    }

    /// Every numeric type is a `number`, so an exact conversion is a no-op on
    /// the representation and only the static type changes.
    fn represent(&self, v: Expr, _from: Prim, _to: Prim) -> Expr {
        v
    }

    fn int_const(&self, v: i128, p: Prim) -> Expr {
        let _ = p;
        Expr::Num(v as f64)
    }

    /// The upper bound, which for `U128` does not fit in an `i128` — casting
    /// it there turns `maxValue` and every `Checked`/`Saturating` bound into
    /// `-1`.
    fn upper_const(&self, v: u128, p: Prim) -> Expr {
        let _ = p;
        Expr::Num(v as f64)
    }

    fn zero(&self, p: Prim) -> Expr {
        let _ = p;
        Expr::Num(0.0)
    }

    fn one(&self, p: Prim) -> Expr {
        let _ = p;
        Expr::Num(1.0)
    }
}
