import Buri.Dynamics.Value

/-!
# Patterns and matching

`Constructor` and `Pattern` are transcriptions of `exhaust.rs:21` and `exhaust.rs:88` --
the *lowered* pattern form the usefulness algorithm works on, not the surface
`hir::Pattern`. `exhaust.rs:93 lower` is what maps one to the other, and it is
where bindings are erased: `PatKind::Bind { sub: None }` becomes `Pattern::Wild`,
because a binding matches everything its sub-pattern does. Exhaustiveness does
not care what a pattern binds, only what it matches.

That erasure is why this file can define matching as `Pattern → Value → Bool`
rather than the `Pattern → Value → Option (List Value)` a full dynamics needs.
Stage 3 will need the binding version; the two agree on `isSome`.
-/

namespace Buri

/-- `exhaust.rs:88`. -/
inductive Pattern where
  | wildcard : Pattern
  | constructor : Constructor → List Pattern → Pattern
  /-- An or-pattern. Expanded into several matrix rows rather than handled by
  the algorithm proper (`exhaust.rs:194 expand`). -/
  | or : List Pattern → Pattern
  deriving Repr

@[elab_as_elim]
theorem Pattern.ind' {motive : Pattern → Prop}
    (wildcard : motive .wildcard)
    (constructor : ∀ c subpatterns, (∀ p ∈ subpatterns, motive p) → motive (.constructor c subpatterns))
    (or : ∀ alternatives, (∀ p ∈ alternatives, motive p) → motive (.or alternatives))
    : ∀ p, motive p :=
  fun p =>
    Pattern.rec (motive_1 := motive) (motive_2 := fun ps => ∀ p ∈ ps, motive p)
      wildcard constructor or
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
  lowers a struct or record-variant pattern to a `subpatterns` vector of length
  `max(field index) + 1`, which can be shorter than the constructor's arity;
  `specialize` (`exhaust.rs:246`) then pads with `Pattern::Wild` and truncates.
  `matchesAll` below is that same convention, so a missing trailing sub-pattern
  matches anything.
* **`arrayRest n` matches at length `n` or above**, checking only the first
  `n` elements. `exhaust.rs:163 expand_lengths` later rewrites it into a
  disjunction of fixed lengths, and `expandLengths_sound` is the proof that
  the rewrite preserves exactly this meaning.
-/

mutual

/-- Does `p` match `v`?

Every constructor except `arrayRest` matches by comparing `constructorOf` and then
recursing into `fields`; `arrayRest n` is the one that matches a *range* of
lengths, checking only the first `n` elements. That asymmetry is the whole
reason `expand_lengths` (`exhaust.rs:163`) exists. -/
def Pattern.matches : Pattern → Value → Bool
  | .wildcard, _ => true
  | .or alternatives, v => Pattern.matchesAny alternatives v
  | .constructor (.arrayRest n) subpatterns, .array vs =>
      decide (n ≤ vs.length) && Pattern.matchesAll subpatterns (vs.take n)
  | .constructor (.arrayRest _) _, _ => false
  | .constructor target subpatterns, v =>
      decide (v.constructorOf = some target) && Pattern.matchesAll subpatterns v.fields

/-- Pointwise matching where a missing pattern is a wildcard. -/
def Pattern.matchesAll : List Pattern → List Value → Bool
  | [], _ => true
  | _ :: _, [] => false
  | p :: ps, v :: vs => Pattern.matches p v && Pattern.matchesAll ps vs

def Pattern.matchesAny : List Pattern → Value → Bool
  | [], _ => false
  | p :: ps, v => Pattern.matches p v || Pattern.matchesAny ps v

end

@[simp] theorem Pattern.matches_wildcard (v : Value) : Pattern.matches .wildcard v = true := by
  simp [Pattern.matches]

@[simp] theorem Pattern.matchesAll_nil (vs : List Value) : Pattern.matchesAll [] vs = true := by
  simp [Pattern.matchesAll]

@[simp] theorem Pattern.matchAny_nil (v : Value) : Pattern.matchesAny [] v = false := by
  simp [Pattern.matchesAny]

theorem Pattern.matchesAny_iff {alternatives : List Pattern} {v : Value} :
    Pattern.matchesAny alternatives v = true ↔ ∃ p ∈ alternatives, Pattern.matches p v = true := by
  induction alternatives with
  | nil => simp
  | cons a as ih =>
    simp only [Pattern.matchesAny, Bool.or_eq_true, ih]
    constructor
    · rintro (h | ⟨p, hp, hm⟩)
      · exact ⟨a, by simp, h⟩
      · exact ⟨p, by simp [hp], hm⟩
    · rintro ⟨p, hp, hm⟩
      rcases List.mem_cons.mp hp with rfl | h'
      · exact .inl hm
      · exact .inr ⟨p, h', hm⟩

/-- A row of the pattern matrix: one pattern per column. -/
abbrev Row := List Pattern

/-- A row matches a value vector when every column does. Column counts always
agree in practice, but as with `matchesAll` a short row is padded with
wildcards, which is what `specialize` produces. -/
def Row.matches : Row → List Value → Bool := Pattern.matchesAll

/-- A value vector is covered by a matrix when some row matches it. This is the
predicate exhaustiveness is really about. -/
def Matrix.covers (P : List Row) (vs : List Value) : Prop :=
  ∃ r ∈ P, Row.matches r vs = true

end Buri
