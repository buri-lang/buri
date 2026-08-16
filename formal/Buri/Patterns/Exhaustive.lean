import Buri.Patterns.Complete

/-!
# The exhaustiveness theorems

The three statements `cli/src/compiler/semantics/exhaustiveness.rs` makes about
a `match`, each a corollary of `isUseful_false_covers`:

* **`exhaustive_correct`** -- if the checker accepts a `match` as exhaustive,
  some arm fires for every value of the scrutinee's type. This is what makes
  `match` progress: evaluation never reaches a scrutinee with no matching arm.
* **`unreachable_correct`** -- if the checker reports an arm unreachable, the
  arms before it really do cover everything it matches, so deleting it changes
  nothing.
* **`irrefutable_correct`** -- if the checker accepts a `let` pattern, it
  matches every value of its type (SPEC 14 rule 2).

All three carry the same two side conditions, and both are honest:

* the arms are **well formed** -- or-free and rest-free, which `expand` and
  `expand_lengths` establish before the algorithm runs; and
* the value is **length-bounded** -- no array longer than `limit`, which is the
  universe `allConstructors` enumerates. `Expand.lean` is where that is
  discharged and the theorems lifted to all values.
-/

namespace Buri

/-- A one-column matrix, which is what a `match` on a single scrutinee is. -/
def armRows (arms : List Pattern) : List Row := arms.map (fun p => [p])

theorem Matrix.WellFormed.armRows {signature : Signature} {t : Ty} {arms : List Pattern}
    (h : ∀ p ∈ arms, Pattern.WellFormed signature t p) :
    Matrix.WellFormed signature [t] (_root_.Buri.armRows arms) := by
  intro row hrow
  obtain ⟨p, hp, rfl⟩ := List.mem_map.mp hrow
  exact ⟨h p hp, trivial⟩

theorem covers_armRows {arms : List Pattern} {v : Value} :
    Matrix.covers (armRows arms) [v] ↔ ∃ p ∈ arms, Pattern.matches p v = true := by
  constructor
  · rintro ⟨row, hrow, hmatch⟩
    obtain ⟨p, hp, rfl⟩ := List.mem_map.mp hrow
    refine ⟨p, hp, ?_⟩
    have : (Pattern.matches p v && Pattern.matchesAll [] ([] : List Value)) = true := hmatch
    simpa using this
  · rintro ⟨p, hp, hmatch⟩
    refine ⟨[p], List.mem_map.mpr ⟨p, hp, rfl⟩, ?_⟩
    show (Pattern.matches p v && Pattern.matchesAll [] ([] : List Value)) = true
    simp [hmatch]

/-- The pattern vector `[wildcard]` is well formed at any single type. -/
theorem Row.WellFormed.singleton_wildcard (signature : Signature) (t : Ty) :
    Row.WellFormed signature [t] [Pattern.wildcard] := ⟨trivial, trivial⟩

/-- **Exhaustiveness is sound.** A `match` the checker accepts has an arm that
fires for every well-typed value of the scrutinee's type.

This is the statement that makes `match` progress -- there is no value at which
evaluation can reach a `match` and find nothing to do. -/
theorem exhaustive_correct (signature : Signature) (limit : Nat) (t : Ty)
    (arms : List Pattern)
    (hwf : ∀ p ∈ arms, Pattern.WellFormed signature t p)
    (hexhaustive : isExhaustive signature limit t (armRows arms) = true) :
    ∀ v, (signature ⊢ᵥ v : t) → Value.Bounded limit v →
      ∃ p ∈ arms, Pattern.matches p v = true := by
  intro v hv hbounded
  refine covers_armRows.mp ?_
  refine isUseful_false_covers signature limit (armRows arms) [Pattern.wildcard] [t]
    ?_ (Matrix.WellFormed.armRows hwf) (Row.WellFormed.singleton_wildcard signature t)
    [v] (.cons hv .nil) ⟨hbounded, trivial⟩ rfl
  simpa [isExhaustive] using hexhaustive

/-- **Unreachability is sound.** An arm the checker calls unreachable matches
nothing the arms before it do not already match. -/
theorem unreachable_correct (signature : Signature) (limit : Nat) (t : Ty)
    (earlier : List Pattern) (arm : Pattern)
    (hearlier : ∀ p ∈ earlier, Pattern.WellFormed signature t p)
    (harm : Pattern.WellFormed signature t arm)
    (hunreachable : isUseful signature limit (armRows earlier) [arm] [t] = false) :
    ∀ v, (signature ⊢ᵥ v : t) → Value.Bounded limit v → Pattern.matches arm v = true →
      ∃ p ∈ earlier, Pattern.matches p v = true := by
  intro v hv hbounded hmatch
  refine covers_armRows.mp ?_
  refine isUseful_false_covers signature limit (armRows earlier) [arm] [t]
    hunreachable (Matrix.WellFormed.armRows hearlier) ⟨harm, trivial⟩
    [v] (.cons hv .nil) ⟨hbounded, trivial⟩ ?_
  show (Pattern.matches arm v && Pattern.matchesAll [] ([] : List Value)) = true
  simp [hmatch]

/-- **Irrefutability is sound.** A `let` pattern the checker accepts matches
every well-typed value of its type (SPEC 14 rule 2). -/
theorem irrefutable_correct (signature : Signature) (limit : Nat) (t : Ty) (p : Pattern)
    (hwf : Pattern.WellFormed signature t p)
    (hirrefutable : isIrrefutable signature limit t p = true) :
    ∀ v, (signature ⊢ᵥ v : t) → Value.Bounded limit v → Pattern.matches p v = true := by
  intro v hv hbounded
  have harms : ∀ q ∈ [p], Pattern.WellFormed signature t q := by
    intro q hq
    rcases List.mem_singleton.mp hq with rfl
    exact hwf
  have := exhaustive_correct signature limit t [p] harms (by
    simpa [isIrrefutable, armRows] using hirrefutable) v hv hbounded
  obtain ⟨q, hq, hm⟩ := this
  rcases List.mem_singleton.mp hq with rfl
  exact hm

end Buri
