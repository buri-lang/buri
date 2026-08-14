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
                let v = a?;
                Some(if from.is_float() {
                    Expr::call(Expr::member(Expr::ident("Math"), "abs"), vec![v])
                } else if from.is_bigint() {
                    // Overflow is a crash, and `abs` of the minimum value
                    // overflows.
                    let neg = Expr::un(UnOp::Neg, v.clone());
                    let cond = Expr::bin(BinOp::Lt, v.clone(), Expr::BigInt("0".into()));
                    self.checked_pub(Expr::cond(cond, neg, v), from)
                } else {
                    let neg = Expr::un(UnOp::Neg, v.clone());
                    let cond = Expr::bin(BinOp::Lt, v.clone(), Expr::Num(0.0));
                    self.checked_pub(Expr::cond(cond, neg, v), from)
                })
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
            // The default `+` crashes on overflow; these are the alternatives,
            // spelled out where they are used.
            "checkedAdd" | "checkedSub" | "checkedMul" | "checkedDiv" => {
                let (x, y) = two();
                let (lo, hi) = from.int_range()?;
                let op = match name {
                    "checkedAdd" => BinOp::Add,
                    "checkedSub" => BinOp::Sub,
                    "checkedMul" => BinOp::Mul,
                    _ => BinOp::Div,
                };
                let raw = if op == BinOp::Div {
                    let zero = self.zero(from);
                    // A checked division by zero is `.None`, not a crash.
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
                        Expr::ident("$none"),
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
                Expr::BigInt(lo.to_string()),
                Expr::BigInt(hi.to_string()),
                Expr::Str(to.name().to_string()),
                Expr::Bool(to.is_bigint()),
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
        if from.is_float() {
            let truncated =
                Expr::call(Expr::member(Expr::ident("Math"), "trunc"), vec![v]);
            let big = Expr::call(Expr::ident("BigInt"), vec![truncated]);
            return self.wrap_bits(big, to);
        }
        let big = if from.is_bigint() {
            v
        } else {
            Expr::call(Expr::ident("BigInt"), vec![v])
        };
        self.wrap_bits(big, to)
    }

    fn wrap_bits(&mut self, big: Expr, to: Prim) -> Expr {
        let f = if to.is_signed() { "asIntN" } else { "asUintN" };
        let wrapped = Expr::call(
            Expr::member(Expr::ident("BigInt"), f),
            vec![Expr::Num(to.bits() as f64), big],
        );
        if to.is_bigint() {
            wrapped
        } else {
            Expr::call(Expr::ident("Number"), vec![wrapped])
        }
    }

    /// Wraps an already-computed value back into its own type's range.
    fn wrap_value(&mut self, v: Expr, p: Prim) -> Expr {
        let big = if p.is_bigint() { v } else { Expr::call(Expr::ident("BigInt"), vec![v]) };
        self.wrap_bits(big, p)
    }

    /// Moves a value between the `number` and `bigint` representations.
    fn represent(&self, v: Expr, from: Prim, to: Prim) -> Expr {
        match (from.is_bigint(), to.is_bigint()) {
            (true, false) => Expr::call(Expr::ident("Number"), vec![v]),
            (false, true) => {
                // A float source has to lose its fraction first, but only an
                // exact conversion reaches here, and those are integral.
                if from.is_float() {
                    Expr::call(
                        Expr::ident("BigInt"),
                        vec![Expr::call(Expr::member(Expr::ident("Math"), "trunc"), vec![v])],
                    )
                } else {
                    Expr::call(Expr::ident("BigInt"), vec![v])
                }
            }
            _ => {
                // `I32 -> F64` and `I64 -> I64` are both no-ops on the
                // representation.
                v
            }
        }
    }

    fn int_const(&self, v: i128, p: Prim) -> Expr {
        if p.is_bigint() {
            Expr::BigInt(v.to_string())
        } else {
            Expr::Num(v as f64)
        }
    }

    /// The upper bound, which for `U128` does not fit in an `i128` — casting
    /// it there turns `maxValue` and every `Checked`/`Saturating` bound into
    /// `-1`.
    fn upper_const(&self, v: u128, p: Prim) -> Expr {
        if p.is_bigint() {
            Expr::BigInt(v.to_string())
        } else {
            Expr::Num(v as f64)
        }
    }

    fn zero(&self, p: Prim) -> Expr {
        if p.is_bigint() {
            Expr::BigInt("0".into())
        } else {
            Expr::Num(0.0)
        }
    }

    fn one(&self, p: Prim) -> Expr {
        if p.is_bigint() {
            Expr::BigInt("1".into())
        } else {
            Expr::Num(1.0)
        }
    }
}
