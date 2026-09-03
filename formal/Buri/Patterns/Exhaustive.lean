import Buri.Patterns.Expand

/-!
# The exhaustiveness theorems

The three statements `cli/src/compiler/semantics/exhaustiveness.rs` makes about
a `match`:

* **`exhaustive_correct`** -- if the checker accepts a `match` as exhaustive,
  some arm fires for every value of the scrutinee's type. This is what makes
  `match` progress: evaluation never reaches a scrutinee with no matching arm.
* **`unreachable_correct`** -- if the checker reports an arm unreachable, the
  arms before it really do cover everything it matches, so deleting it changes
  nothing.
* **`irrefutable_correct`** -- if the checker accepts a `let` pattern, it
  matches every value of its type (design/static-rules.md rule 2).

Each is a corollary of `isUseful_false_covers`, stated over the *compiled*
arms and inside the bounded universe -- values whose arrays are no shorter
than `limit`. `exhaustive_correct_unbounded`, at the bottom, removes both
caveats and states the theorem a Buri programmer would recognise:

> If the compiler accepts a `match` as exhaustive, then for **every** value of
> the scrutinee's type -- no length restriction -- some arm the programmer
> wrote fires.

Three results combine there: `exhaustive_correct` (the algorithm is right
inside the bounded universe), `Pattern.matches_truncate` (no arm can tell a
long array from its truncation), and `expandLengths_sound` /
`topDisjuncts_sound` (the two rewrites the compiler applies first only ever
*narrow* what a pattern matches). The value is truncated into the bounded
universe, the algorithm finds a compiled arm covering the truncation,
soundness carries that back to a source arm, and the representative lemma
carries it from the truncation to the original value.
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
every well-typed value of its type (design/static-rules.md rule 2). -/
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

/-- The arm list the algorithm actually runs on: rest patterns rewritten into
fixed lengths, then top-level alternations split into separate rows. This is
`expand(expand_lengths(..))` from `exhaustiveness.rs`'s `check`. -/
def compileArms (limit : Nat) (arms : List Pattern) : List Pattern :=
  Pattern.topDisjunctsList (arms.map (Pattern.expandLengths limit))

/-- Every compiled arm matches only what some source arm matches. -/
theorem compileArms_sound (limit : Nat) (arms : List Pattern)
    (hlowered : ∀ p ∈ arms, Pattern.LoweredArrays p) :
    ∀ q ∈ compileArms limit arms, ∀ w, Pattern.matches q w = true →
      ∃ p ∈ arms, Pattern.matches p w = true := by
  intro q hq w hm
  obtain ⟨e, he, hqe⟩ := Pattern.mem_topDisjunctsList hq
  obtain ⟨p, hp, rfl⟩ := List.mem_map.mp he
  exact ⟨p, hp, Pattern.expandLengths_sound limit p w (hlowered p hp)
    (Pattern.topDisjuncts_sound _ q w hqe hm)⟩

/-- **Exhaustiveness, without the array-length restriction.**

If the checker accepts the compiled arms as exhaustive, then every well-typed
value of the scrutinee's type is matched by one of the *source* arms.

The hypotheses are exactly what the compiler establishes before calling the
algorithm:

* `hlowered` -- `lower` sizes an array constructor by its sub-pattern count;
* `hlen` -- `limit` is chosen larger than every array length any arm mentions
  (`exhaustiveness.rs` computes `max(length_limit) + 1`);
* `hwf` -- the compiled arms are rest-free and respect constructor arities. -/
theorem exhaustive_correct_unbounded (signature : Signature) (limit : Nat) (t : Ty)
    (arms : List Pattern)
    (hlowered : ∀ p ∈ arms, Pattern.LoweredArrays p)
    (hlen : ∀ p ∈ arms, p.lengthLimit < limit)
    (hwf : ∀ q ∈ compileArms limit arms, Pattern.WellFormed signature t q)
    (hexhaustive : isExhaustive signature limit t (armRows (compileArms limit arms)) = true) :
    ∀ v, (signature ⊢ᵥ v : t) → ∃ p ∈ arms, Pattern.matches p v = true := by
  intro v hv
  -- Step into the bounded universe.
  have htruncTyped := HasType.truncate limit v t hv
  have htruncBounded := Value.truncate_bounded limit v
  obtain ⟨q, hq, hqm⟩ := exhaustive_correct signature limit t (compileArms limit arms)
    hwf hexhaustive (Value.truncate limit v) htruncTyped htruncBounded
  -- Carry the match back to a source arm...
  obtain ⟨e, he, hqe⟩ := Pattern.mem_topDisjunctsList hq
  obtain ⟨p, hp, rfl⟩ := List.mem_map.mp he
  refine ⟨p, hp, ?_⟩
  have hExpanded : Pattern.matches (p.expandLengths limit) (Value.truncate limit v) = true :=
    Pattern.topDisjuncts_sound _ q _ hqe hqm
  have hSource : Pattern.matches p (Value.truncate limit v) = true :=
    Pattern.expandLengths_sound limit p _ (hlowered p hp) hExpanded
  -- ...and then from the truncation to the original value.
  rwa [Pattern.matches_truncate limit p v (hlen p hp)] at hSource

/-- **Irrefutability, without the array-length restriction.** A `let` pattern
the checker accepts matches every well-typed value of its type (design/static-rules.md
rule 2). The one-armed case of the theorem above, which is exactly how the
compiler asks the question: `check` runs the same usefulness algorithm on a
one-row matrix. -/
theorem irrefutable_correct_unbounded (signature : Signature) (limit : Nat) (t : Ty)
    (p : Pattern)
    (hlowered : Pattern.LoweredArrays p)
    (hlen : p.lengthLimit < limit)
    (hwf : ∀ q ∈ compileArms limit [p], Pattern.WellFormed signature t q)
    (hexhaustive : isExhaustive signature limit t (armRows (compileArms limit [p])) = true) :
    ∀ v, (signature ⊢ᵥ v : t) → Pattern.matches p v = true := by
  intro v hv
  obtain ⟨q, hq, hm⟩ := exhaustive_correct_unbounded signature limit t [p]
    (fun r hr => by rcases List.mem_singleton.mp hr with rfl; exact hlowered)
    (fun r hr => by rcases List.mem_singleton.mp hr with rfl; exact hlen)
    hwf hexhaustive v hv
  rcases List.mem_singleton.mp hq with rfl
  exact hm

end Buri
