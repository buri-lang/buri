//! The methods and trait impls the primitive types carry.
//!
//! SPEC 6.2.1 says there are a lot of these in `core/num` — one per
//! source-and-target pair — and that they are ordinary methods rather than
//! cast operators. They are registered here rather than written out in Buri
//! for one reason: `i32.toI64()` and `i16.toI64()` are different operations
//! that share a name, which is fine for a method (resolved by the receiver's
//! type) and would be overloading for a free function. Registering them
//! directly keeps them methods and keeps `core/num`'s module scope honest.

use crate::compiler::semantics::resolve::Checker;
use crate::compiler::semantics::types::*;
use crate::diagnostics::{Invariant as _, Span};

impl<'a> Checker<'a> {
    pub(crate) fn register_primitive_methods(&mut self) {
        let numeric: Vec<Prim> = Prim::all()
            .iter()
            .copied()
            .filter(|p| p.is_integer() || p.is_float())
            .collect();

        for &p in &numeric {
            self.numeric_conversions(p, &numeric);
            self.numeric_methods(p);
        }
        self.char_methods();
        self.comparison_impls();
        self.json_impls();
        self.assert_i64_is_trait_maximal();
    }

    /// `ToJson` and `FromJson` where the fold bottoms out.
    ///
    /// `derive ToJson for Point` asks whether `Int` satisfies `ToJson`, and
    /// the answer has to be in the impl table — so the primitives get theirs
    /// here, the way they get `Show`'s. `Option` gets one too, marked derived,
    /// because `Option<T>` encodes when `T` does and the descriptor already
    /// carries the payload's shape.
    ///
    /// All of it is conditional on `core/json` being loaded, which it is
    /// exactly when a program could have named either trait: the module loads
    /// on import, and there is no way to write `ToJson` without importing it.
    fn json_impls(&mut self) {
        if !self.known_traits.contains_key("ToJson") {
            return;
        }
        let Some(&json) = self.known_types.get("Json") else { return };
        let json_ty = Ty::Con(json, Vec::new());
        let decoded = |me: &Self, p: Prim| -> Ty {
            let (Some(&result), Some(&err)) =
                (me.known_types.get("Result"), me.known_types.get("DecodeError"))
            else {
                return Ty::Error;
            };
            Ty::Con(result, vec![me.tables.prim(p), Ty::Con(err, Vec::new())])
        };

        let mut prims: Vec<Prim> = Prim::all()
            .iter()
            .copied()
            .filter(|p| p.is_integer() || p.is_float())
            .collect();
        prims.extend([Prim::Char, Prim::Str, Prim::Bool]);
        for p in prims {
            let con = self.tables.prim_id(p);
            let to = self.declare_to_json(p, json_ty.clone());
            let ret = decoded(self, p);
            let from = self.declare_from_json(p, json_ty.clone(), ret);
            self.add_impl("ToJson", con, vec![to]);
            self.add_impl("FromJson", con, vec![from]);
        }

        // `Option` is `derive`d rather than given methods: `.None` is `null`
        // and `.Some(x)` is whatever `x` is, which is a fold over the payload
        // and so exactly what a derived implementation means.
        if let Some(&option) = self.known_types.get("Option") {
            self.add_derived_impl("ToJson", option);
            self.add_derived_impl("FromJson", option);
        }
    }

    /// `fn toJson<C: Alloc>(self, ctx: C): Json` — `Show.show`'s shape,
    /// because encoding allocates for the same reason rendering does.
    fn declare_to_json(&mut self, p: Prim, json_ty: Ty) -> FnId {
        let con = self.tables.prim_id(p);
        let module = self.prim_module_of(p);
        let alloc = self.known_traits.get("Alloc").copied();
        let generics = vec![GenericInfo {
            name: "C".into(),
            bounds: alloc.into_iter().collect(),
            span: Span::NONE,
        }];
        let fid = self.tables.add_fn(FnInfo {
            name: "toJson".into(),
            module,
            generics,
            params: vec![
                ParamInfo {
                    name: "self".into(),
                    ty: Ty::Con(con, Vec::new()),
                    role: ParamRole::SelfParam,
                    span: Span::NONE,
                },
                ParamInfo {
                    name: "ctx".into(),
                    ty: Ty::Param(0),
                    role: ParamRole::Ctx,
                    span: Span::NONE,
                },
            ],
            ret: json_ty,
            exported: true,
            span: Span::NONE,
            self_ty: Some(con),
            impl_of: None,
            ast: AstRef::Builtin,
            intrinsic: true,
        });
        self.tables.add_method(con, "toJson", fid);
        fid
    }

    /// `fn fromJson(value: Json): Result<Self, DecodeError>`.
    ///
    /// It takes no `self` — there is no value yet — so like `Bounded`'s
    /// methods it gets no entry in the method table. `json.decode` reaches it
    /// through the type it is asked for.
    fn declare_from_json(&mut self, p: Prim, json_ty: Ty, ret: Ty) -> FnId {
        let con = self.tables.prim_id(p);
        let module = self.prim_module_of(p);
        self.tables.add_fn(FnInfo {
            name: "fromJson".into(),
            module,
            generics: Vec::new(),
            params: vec![ParamInfo {
                name: "value".into(),
                ty: json_ty,
                role: ParamRole::Normal,
                span: Span::NONE,
            }],
            ret,
            exported: true,
            span: Span::NONE,
            self_ty: Some(con),
            impl_of: None,
            ast: AstRef::Builtin,
            intrinsic: true,
        })
    }

    /// **Principality rests on this.** `satisfies` answers `true` for an
    /// unresolved type variable, so a bound on a numeric literal is discharged
    /// only *after* `default_numerics` has committed that literal to `I64`.
    /// The algorithm therefore commits to a type before it checks the bound,
    /// and the only thing that keeps that from being a principality
    /// counterexample is that there is no bound `I64` fails and another
    /// integer type satisfies.
    ///
    /// That is true today and nothing states it: `I64` gets `Neg`, which the
    /// unsigned types lack, and every other trait a primitive gets is given to
    /// all of them alike. SPEC 14 rule 22 keeps a user from adding to another
    /// type's set from outside its defining module, so the set cannot be
    /// widened later — but a new trait added *here*, given to `U64` and not to
    /// `I64`, would silently break defaulting. This fails the build instead.
    ///
    /// The check reads the real impl table rather than a list, so it cannot
    /// drift from what was registered.
    fn assert_i64_is_trait_maximal(&self) {
        let traits_of = |p: Prim| crate::compiler::semantics::types::traits_of(&self.tables, self.tables.prim_id(p));
        let i64_traits = traits_of(Prim::I64);
        for &p in Prim::all() {
            if !p.is_integer() || p == Prim::I64 {
                continue;
            }
            let missing: Vec<String> =
                traits_of(p).difference(&i64_traits).cloned().collect();
            assert!(
                missing.is_empty(),
                "principality depends on `I64` being trait-maximal among the integer types, \
                 because defaulting commits a literal to `I64` before any bound on it is \
                 checked — but `{}` implements {missing:?}, which `I64` does not. Either give \
                 `I64` the trait too, or make bound checking precede defaulting.",
                p.name(),
            );
        }
    }

    fn prim_module_of(&self, p: Prim) -> ModuleId {
        self.loaded
            .find(crate::compiler::standard_library::defining_module(p))
            .unwrap_or(ModuleId(0))
    }

    /// Declares an intrinsic method on a primitive.
    fn method(&mut self, on: Prim, name: &str, params: Vec<Ty>, ret: Ty) -> FnId {
        let con = self.tables.prim_id(on);
        let module = self.prim_module_of(on);
        let mut infos = vec![ParamInfo {
            name: "self".into(),
            ty: Ty::Con(con, Vec::new()),
            role: ParamRole::SelfParam,
            span: Span::NONE,
        }];
        for (i, t) in params.into_iter().enumerate() {
            infos.push(ParamInfo {
                name: format!("a{i}"),
                ty: t,
                role: ParamRole::Normal,
                span: Span::NONE,
            });
        }
        let fid = self.tables.add_fn(FnInfo {
            name: name.to_string(),
            module,
            generics: Vec::new(),
            params: infos,
            ret,
            exported: true,
            span: Span::NONE,
            self_ty: Some(con),
            impl_of: None,
            ast: AstRef::Builtin,
            intrinsic: true,
        });
        self.tables.add_method(con, name, fid);
        fid
    }

    /// Three families, distinguished by what happens when the value does not
    /// fit — and the return type is what says which (SPEC 6.2.1).
    fn numeric_conversions(&mut self, from: Prim, all: &[Prim]) {
        let range_error = self
            .known_types
            .get("RangeError")
            .map(|c| Ty::Con(*c, Vec::new()))
            .unwrap_or(Ty::Error);
        let result = self.known_types.get("Result").copied();

        for &to in all {
            let name = format!("to{}", to.name());
            let target = self.tables.prim(to);
            let exact = conversion_is_exact(from, to);
            let ret = if exact {
                target.clone()
            } else {
                match result {
                    Some(r) => Ty::Con(r, vec![target.clone(), range_error.clone()]),
                    None => Ty::Error,
                }
            };
            self.method(from, &name, Vec::new(), ret);

            // The modular form: wraps for integers, rounds for floats. For
            // wire formats and checksums, where wrapping is the intent.
            if !exact {
                self.method(from, &format!("wrapTo{}", to.name()), Vec::new(), target);
            }
        }
    }

    fn numeric_methods(&mut self, p: Prim) {
        let self_ty = self.tables.prim(p);
        let bool_ty = self.tables.prim(Prim::Bool);
        let str_ty = self.tables.prim(Prim::Str);
        let u64_ty = self.tables.prim(Prim::U64);
        let order = self.known_types.get("Order").map(|c| Ty::Con(*c, Vec::new()));
        let option = self.known_types.get("Option").copied();

        if p.is_signed() || p.is_float() {
            self.method(p, "abs", Vec::new(), self_ty.clone());
            self.method(p, "signum", Vec::new(), self_ty.clone());
        }

        // The operator traits. `a + b` on two operands of the same primitive
        // compiles to the operation directly; these exist so a bound like
        // `<N: Add>` is satisfiable by a primitive.
        let mut op_methods: Vec<(&str, FnId)> = Vec::new();
        for op in ["add", "sub", "mul", "div", "rem"] {
            let fid = self.method(p, op, vec![self_ty.clone()], self_ty.clone());
            op_methods.push((op, fid));
        }
        if p.is_signed() || p.is_float() {
            let fid = self.method(p, "neg", Vec::new(), self_ty.clone());
            op_methods.push(("neg", fid));
        }

        let eq = self.method(p, "eq", vec![self_ty.clone()], bool_ty);
        let compare =
            order.as_ref().map(|o| self.method(p, "compare", vec![self_ty.clone()], o.clone()));
        // Rendering allocates, so `show` names `Alloc`.
        let show = self.show_method(p, str_ty);
        let hash = self.method(p, "hash", Vec::new(), u64_ty);

        let mut checked = Vec::new();
        let mut wrapping = Vec::new();
        let mut saturating = Vec::new();
        if p.is_integer() {
            if let Some(opt) = option {
                let opt_self = Ty::Con(opt, vec![self_ty.clone()]);
                for name in ["checkedAdd", "checkedSub", "checkedMul", "checkedDiv"] {
                    checked.push(self.method(p, name, vec![self_ty.clone()], opt_self.clone()));
                }
            }
            for name in ["wrappingAdd", "wrappingSub", "wrappingMul"] {
                wrapping.push(self.method(p, name, vec![self_ty.clone()], self_ty.clone()));
            }
            for name in ["saturatingAdd", "saturatingSub", "saturatingMul"] {
                saturating.push(self.method(p, name, vec![self_ty.clone()], self_ty.clone()));
            }
        }

        // `Bounded`'s methods take no `self`, so they get no entry in the
        // method table; `num.minValue<U8>()` reaches them.
        let con = self.tables.prim_id(p);
        let bounded_methods = vec![
            self.static_method(p, "minValue", self_ty.clone()),
            self.static_method(p, "maxValue", self_ty.clone()),
        ];

        self.add_impl("Eq", con, vec![eq]);
        if let Some(c) = compare {
            self.add_impl("Ord", con, vec![c]);
        }
        self.add_impl("Show", con, vec![show]);
        self.add_impl("Hash", con, vec![hash]);
        for (name, fid) in op_methods {
            let trait_name = match name {
                "add" => "Add",
                "sub" => "Sub",
                "mul" => "Mul",
                "div" => "Div",
                "rem" => "Rem",
                _ => "Neg",
            };
            self.add_impl(trait_name, con, vec![fid]);
        }
        self.add_impl("Bounded", con, bounded_methods);
        if !checked.is_empty() {
            self.add_impl("Checked", con, checked);
        }
        if !wrapping.is_empty() {
            self.add_impl("Wrapping", con, wrapping);
        }
        if !saturating.is_empty() {
            self.add_impl("Saturating", con, saturating);
        }
    }

    /// `fn show<C: Alloc>(self, ctx: C): Str`
    fn show_method(&mut self, p: Prim, str_ty: Ty) -> FnId {
        let con = self.tables.prim_id(p);
        let module = self.prim_module_of(p);
        let alloc = self.known_traits.get("Alloc").copied();
        let generics = vec![GenericInfo {
            name: "C".into(),
            bounds: alloc.into_iter().collect(),
            span: Span::NONE,
        }];
        let fid = self.tables.add_fn(FnInfo {
            name: "show".into(),
            module,
            generics,
            params: vec![
                ParamInfo {
                    name: "self".into(),
                    ty: Ty::Con(con, Vec::new()),
                    role: ParamRole::SelfParam,
                    span: Span::NONE,
                },
                ParamInfo {
                    name: "ctx".into(),
                    ty: Ty::Param(0),
                    role: ParamRole::Ctx,
                    span: Span::NONE,
                },
            ],
            ret: str_ty,
            exported: true,
            span: Span::NONE,
            self_ty: Some(con),
            impl_of: None,
            ast: AstRef::Builtin,
            intrinsic: true,
        });
        self.tables.add_method(con, "show", fid);
        fid
    }

    /// A trait method with no `self`, reached through a free function rather
    /// than a receiver.
    fn static_method(&mut self, p: Prim, name: &str, ret: Ty) -> FnId {
        let con = self.tables.prim_id(p);
        let module = self.prim_module_of(p);
        self.tables.add_fn(FnInfo {
            name: name.to_string(),
            module,
            generics: Vec::new(),
            params: Vec::new(),
            ret,
            exported: true,
            span: Span::NONE,
            self_ty: Some(con),
            impl_of: None,
            ast: AstRef::Builtin,
            intrinsic: true,
        })
    }

    fn char_methods(&mut self) {
        // "`Char` and `U32` convert the same way: `c.toU32()` is exact,
        // `n.toChar()` yields `Result<Char, RangeError>`" — not every `U32` is
        // a Unicode scalar value.
        let char_ty = self.tables.prim(Prim::Char);
        let range_error = self
            .known_types
            .get("RangeError")
            .map(|c| Ty::Con(*c, Vec::new()))
            .unwrap_or(Ty::Error);
        if let Some(result) = self.known_types.get("Result").copied() {
            let ret = Ty::Con(result, vec![char_ty, range_error]);
            self.method(Prim::U32, "toChar", Vec::new(), ret);
        }

        let char_con = self.tables.prim_id(Prim::Char);
        let bool_ty = self.tables.prim(Prim::Bool);
        let str_ty = self.tables.prim(Prim::Str);
        let u64_ty = self.tables.prim(Prim::U64);
        let order = self.known_types.get("Order").map(|c| Ty::Con(*c, Vec::new()));

        let eq = self.method(Prim::Char, "eq", vec![self_of(self, Prim::Char)], bool_ty.clone());
        let show = self.show_method(Prim::Char, str_ty.clone());
        let hash = self.method(Prim::Char, "hash", Vec::new(), u64_ty.clone());
        self.add_impl("Eq", char_con, vec![eq]);
        self.add_impl("Show", char_con, vec![show]);
        self.add_impl("Hash", char_con, vec![hash]);
        if let Some(o) = &order {
            let cmp =
                self.method(Prim::Char, "compare", vec![self_of(self, Prim::Char)], o.clone());
            self.add_impl("Ord", char_con, vec![cmp]);
        }

        // Str, Bool and Template.
        for p in [Prim::Str, Prim::Bool] {
            let con = self.tables.prim_id(p);
            let eq = self.method(p, "eq", vec![self_of(self, p)], bool_ty.clone());
            let show = self.show_method(p, str_ty.clone());
            let hash = self.method(p, "hash", Vec::new(), u64_ty.clone());
            self.add_impl("Eq", con, vec![eq]);
            self.add_impl("Show", con, vec![show]);
            self.add_impl("Hash", con, vec![hash]);
            if let Some(o) = &order {
                let cmp = self.method(p, "compare", vec![self_of(self, p)], o.clone());
                self.add_impl("Ord", con, vec![cmp]);
            }
        }
    }

    fn comparison_impls(&mut self) {}

    /// `methods` is in the trait's declaration order. The slot vector is sized
    /// from the trait itself rather than from the argument, so a caller that
    /// supplies too few or too many cannot leave the table holding a row of a
    /// length no consumer expects.
    fn add_impl(&mut self, trait_name: &str, con: TyConId, methods: Vec<FnId>) {
        let Some(&trait_id) = self.known_traits.get(trait_name) else { return };
        let declared = self.tables.trait_(trait_id).methods.len();
        let mut slots: Vec<Option<FnId>> = vec![None; declared];
        for (slot, fid) in slots.iter_mut().zip(methods) {
            *slot = Some(fid);
        }
        let head = self.tables.generic_head(con);
        self.tables.add_impl(ImplInfo {
            trait_id,
            self_con: con,
            head,
            body: ImplBody::Written(slots),
            span: Span::NONE,
        });
    }

    /// The same, marked `derived`: the implementation is the fold over the
    /// type's components rather than a body, so it has no methods and
    /// `satisfies` recurses into the payload before answering.
    fn add_derived_impl(&mut self, trait_name: &str, con: TyConId) {
        let Some(&trait_id) = self.known_traits.get(trait_name) else { return };
        let head = self.tables.generic_head(con);
        self.tables.add_impl(ImplInfo {
            trait_id,
            self_con: con,
            head,
            body: ImplBody::Derived,
            span: Span::NONE,
        });
    }
}

fn self_of(c: &Checker<'_>, p: Prim) -> Ty {
    c.tables.prim(p)
}

/// Whether every value of `from` is representable in `to`.
///
/// `I64 -> F64` is lossy above 2^53, so strictly it belongs in the fallible
/// family — but converting a count to a float is too common to route through a
/// `Result`, so it is exact-to-53-bits and documented as such. That is the one
/// place the language prefers ergonomics to ceremony (SPEC 6.2.1).
pub fn conversion_is_exact(from: Prim, to: Prim) -> bool {
    if from == to {
        return true;
    }
    match (from.is_integer(), to.is_integer()) {
        (true, true) => {
            let claim = "int_range answers for every integer type, which this arm is reached \
                         only for";
            let (from_lo, from_hi) = from.int_range().or_ice(claim);
            let (to_lo, to_hi) = to.int_range().or_ice(claim);
            from_lo >= to_lo && from_hi <= to_hi
        }
        // Integer to float: always allowed, exact to 53 bits.
        (true, false) => true,
        // Float to integer: never exact — it can be fractional, infinite, or
        // out of range.
        (false, true) => false,
        // F32 widens exactly; F64 narrows lossily.
        (false, false) => from == Prim::F32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widening_is_exact_and_narrowing_is_not() {
        assert!(conversion_is_exact(Prim::I32, Prim::I64));
        assert!(!conversion_is_exact(Prim::I64, Prim::I32));
        assert!(conversion_is_exact(Prim::U8, Prim::I16));
        // U8 does not fit in I8: 255 is out of range.
        assert!(!conversion_is_exact(Prim::U8, Prim::I8));
        // No unsigned type holds a negative value.
        assert!(!conversion_is_exact(Prim::I8, Prim::U8));
        assert!(conversion_is_exact(Prim::F32, Prim::F64));
        assert!(!conversion_is_exact(Prim::F64, Prim::F32));
        assert!(conversion_is_exact(Prim::I64, Prim::F64));
        assert!(!conversion_is_exact(Prim::F64, Prim::I64));
    }
}
