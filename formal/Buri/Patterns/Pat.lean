import Buri.Dynamics.Value

/-!
# Patterns and matching

`Ctor` and `Pat` are transcriptions of `exhaust.rs:21` and `exhaust.rs:88` --
the *lowered* pattern form the usefulness algorithm works on, not the surface
`hir::Pattern`. `exhaust.rs:93 lower` is what maps one to the other, and it is
where bindings are erased: `PatKind::Bind { sub: None }` becomes `Pat::Wild`,
because a binding matches everything its sub-pattern does. Exhaustiveness does
not care what a pattern binds, only what it matches.

That erasure is why this file can define matching as `Pat → Value → Bool`
rather than the `Pat → Value → Option (List Value)` a full dynamics needs.
Stage 3 will need the binding version; the two agree on `isSome`.
-/

namespace Buri

/-- The head constructor of a pattern (`exhaust.rs:21`). -/
inductive Ctor where
  | variant : TyConId → Nat → Ctor
  /-- Structs, tuples and unit: exactly one constructor. -/
  | single : Ctor
  | bool : Bool → Ctor
  /-- A fixed-length array pattern. -/
  | array : Nat → Ctor
  /-- `[a, b, ..rest]` -- matches any length at or above `n`. -/
  | arrayRest : Nat → Ctor
  /-- A literal drawn from a set too large to enumerate. Two different
  literals are two different constructors, and no finite set of them ever
  completes a match. -/
  | lit : String → Ctor
  deriving DecidableEq, Repr

/-- `exhaust.rs:88`. -/
inductive Pat where
  | wild : Pat
  | ctor : Ctor → List Pat → Pat
  /-- An or-pattern. Expanded into several matrix rows rather than handled by
  the algorithm proper (`exhaust.rs:194 expand`). -/
  | or : List Pat → Pat
  deriving Repr

@[elab_as_elim]
theorem Pat.ind' {motive : Pat → Prop}
    (wild : motive .wild)
    (ctor : ∀ c subs, (∀ p ∈ subs, motive p) → motive (.ctor c subs))
    (or : ∀ alts, (∀ p ∈ alts, motive p) → motive (.or alts))
    : ∀ p, motive p :=
  fun p =>
    Pat.rec (motive_1 := motive) (motive_2 := fun ps => ∀ p ∈ ps, motive p)
      wild ctor or
      (fun _ h => absurd h List.not_mem_nil)
      (fun _ _ ihHd ihTl a ha => by
        rcases List.mem_cons.mp ha with rfl | h'
        · exact ihHd
        · exact ihTl a h')
      p

/-!
## Matching

Two details are inherited from `exhaust.rs` rather than invented here, and
both matter for the theorems:

* **Sub-patterns are padded, not required to be complete.** `exhaust.rs:114`
  lowers a struct or record-variant pattern to a `subs` vector of length
  `max(field index) + 1`, which can be shorter than the constructor's arity;
  `specialize` (`exhaust.rs:246`) then pads with `Pat::Wild` and truncates.
  `matchPad` below is that same convention, so a missing trailing sub-pattern
  matches anything.
* **`arrayRest n` matches at length `n` or above**, checking only the first
  `n` elements. `exhaust.rs:163 expand_lengths` later rewrites it into a
  disjunction of fixed lengths, and `expandLengths_sound` is the proof that
  the rewrite preserves exactly this meaning.
-/

/-- The constructor a value is built with, or `none` for the values no pattern
can inspect. This is the projection that makes `specialize` correct: a row
survives specialisation on `ct` exactly when the scrutinee's `ctorOf` is `ct`.

An array's constructor records its *length*, which is what makes
`Ctor.array n` a fixed-length test. -/
def Value.ctorOf : Value → Option Ctor
  | .variant c i _ => some (.variant c i)
  | .single _ => some .single
  | .bool b => some (.bool b)
  | .array vs => some (.array vs.length)
  | .lit s => some (.lit s)
  | .opq => none

/-- The sub-values a constructor holds. -/
def Value.fieldsOf : Value → List Value
  | .variant _ _ vs => vs
  | .single vs => vs
  | .bool _ => []
  | .array vs => vs
  | .lit _ => []
  | .opq => []

mutual

/-- Does `p` match `v`?

Every constructor except `arrayRest` matches by comparing `ctorOf` and then
recursing into `fieldsOf`; `arrayRest n` is the one that matches a *range* of
lengths, checking only the first `n` elements. That asymmetry is the whole
reason `expand_lengths` (`exhaust.rs:163`) exists. -/
def Pat.matches : Pat → Value → Bool
  | .wild, _ => true
  | .or alts, v => Pat.matchAny alts v
  | .ctor (.arrayRest n) subs, .array vs =>
      decide (n ≤ vs.length) && Pat.matchPad subs (vs.take n)
  | .ctor (.arrayRest _) _, _ => false
  | .ctor ct subs, v =>
      decide (v.ctorOf = some ct) && Pat.matchPad subs v.fieldsOf

/-- Pointwise matching where a missing pattern is a wildcard. -/
def Pat.matchPad : List Pat → List Value → Bool
  | [], _ => true
  | _ :: _, [] => false
  | p :: ps, v :: vs => Pat.matches p v && Pat.matchPad ps vs

def Pat.matchAny : List Pat → Value → Bool
  | [], _ => false
  | p :: ps, v => Pat.matches p v || Pat.matchAny ps v

end

@[simp] theorem Pat.matches_wild (v : Value) : Pat.matches .wild v = true := by
  simp [Pat.matches]

@[simp] theorem Pat.matchPad_nil (vs : List Value) : Pat.matchPad [] vs = true := by
  simp [Pat.matchPad]

@[simp] theorem Pat.matchAny_nil (v : Value) : Pat.matchAny [] v = false := by
  simp [Pat.matchAny]

theorem Pat.matchAny_iff {alts : List Pat} {v : Value} :
    Pat.matchAny alts v = true ↔ ∃ p ∈ alts, Pat.matches p v = true := by
  induction alts with
  | nil => simp
  | cons a as ih =>
    simp only [Pat.matchAny, Bool.or_eq_true, ih]
    constructor
    · rintro (h | ⟨p, hp, hm⟩)
      · exact ⟨a, by simp, h⟩
      · exact ⟨p, by simp [hp], hm⟩
    · rintro ⟨p, hp, hm⟩
      rcases List.mem_cons.mp hp with rfl | h'
      · exact .inl hm
      · exact .inr ⟨p, h', hm⟩

/-- A row of the pattern matrix: one pattern per column. -/
abbrev Row := List Pat

/-- A row matches a value vector when every column does. Column counts always
agree in practice, but as with `matchPad` a short row is padded with
wildcards, which is what `specialize` produces. -/
def Row.matches : Row → List Value → Bool := Pat.matchPad

/-- A value vector is covered by a matrix when some row matches it. This is the
predicate exhaustiveness is really about. -/
def Matrix.covers (P : List Row) (vs : List Value) : Prop :=
  ∃ r ∈ P, Row.matches r vs = true

end Buri
