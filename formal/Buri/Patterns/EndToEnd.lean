import Buri.Patterns.Expand

/-!
# Exhaustiveness, end to end

`exhaustive_correct` in `Exhaustive.lean` is stated over the *compiled* arms
and only for values whose arrays are no longer than `limit`. This file removes
both caveats and states the theorem a Buri programmer would recognise:

> If the compiler accepts a `match` as exhaustive, then for **every** value of
> the scrutinee's type -- no length restriction -- some arm the programmer
> wrote fires.

Three results combine:

* `exhaustive_correct` -- the algorithm is right inside the bounded universe;
* `Pattern.matches_truncate` -- no arm can tell a long array from its
  truncation, because `limit` exceeds every length any arm mentions; and
* `expandLengths_sound` / `topDisjuncts_sound` -- the two rewrites the compiler
  applies first only ever *narrow* what a pattern matches.

The value is truncated into the bounded universe, the algorithm finds a
compiled arm covering the truncation, soundness carries that back to a source
arm, and the representative lemma carries it from the truncation to the
original value.
-/

namespace Buri

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

end Buri
