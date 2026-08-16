import Buri.Patterns.Truncate

/-!
# The two rewrites that run before the algorithm

`exhaustiveness.rs`'s `check` does not hand the arms to `useful` directly. It
first applies

* `expand_lengths`, rewriting `[a, ..rest]` into a disjunction of fixed lengths
  `[a] | [a, _] | ... ` up to `limit`; and
* `expand`, splitting a top-level alternation into separate matrix rows.

Only their **soundness** is needed here -- that anything a rewritten pattern
matches, the original matches too. That is the direction the exhaustiveness
theorem consumes: the algorithm finds a rewritten arm covering the value, and
soundness carries that back to a real arm the programmer wrote.

## `lower`'s invariant

`Pattern.LoweredArrays` records something `lower` guarantees and the rewrite
depends on: an array constructor's sub-pattern count *equals* its index,
because `lower` builds it as `Ctor::Array(subs.len())`. Without that,
`expand_lengths` could pad a rest pattern into a fixed-length pattern that
tests more positions than the rest pattern did.

## One deliberate difference from the Rust

`expand_lengths` returns `alts[0]` rather than `Or(alts)` when there is exactly
one alternative. That is a node-count optimisation with no semantic content --
`matchesAny [x] v = matches x v` -- so this model always builds the `or`. Every
consumer here goes through `matches`, so the two agree.
-/

namespace Buri

/-! ## What `lower` guarantees -/

mutual

/-- An array constructor's sub-pattern count equals its index. `lower` builds
`Ctor::Array(subs.len())` and `Ctor::ArrayRest(subs.len())`, so this holds of
everything the algorithm ever sees. -/
def Pattern.LoweredArrays : Pattern → Prop
  | .wildcard => True
  | .or alternatives => Pattern.LoweredArraysList alternatives
  | .constructor (.array n) subpatterns =>
      subpatterns.length = n ∧ Pattern.LoweredArraysList subpatterns
  | .constructor (.arrayRest n) subpatterns =>
      subpatterns.length = n ∧ Pattern.LoweredArraysList subpatterns
  | .constructor _ subpatterns => Pattern.LoweredArraysList subpatterns

def Pattern.LoweredArraysList : List Pattern → Prop
  | [] => True
  | p :: ps => Pattern.LoweredArrays p ∧ Pattern.LoweredArraysList ps

end

theorem Pattern.LoweredArraysList.mem :
    ∀ {ps : List Pattern} {p : Pattern},
      Pattern.LoweredArraysList ps → p ∈ ps → Pattern.LoweredArrays p := by
  intro ps
  induction ps with
  | nil => intro p _ hp; exact absurd hp List.not_mem_nil
  | cons q qs ih =>
    intro p h hp
    simp only [Pattern.LoweredArraysList] at h
    rcases List.mem_cons.mp hp with rfl | hp'
    · exact h.1
    · exact ih h.2 hp'

/-! ## `expand_lengths` -/

mutual

/-- `expand_lengths` from `exhaustiveness.rs`. -/
def Pattern.expandLengths (limit : Nat) : Pattern → Pattern
  | .wildcard => .wildcard
  | .or alternatives => .or (Pattern.expandLengthsList limit alternatives)
  | .constructor (.arrayRest n) subpatterns =>
      .or ((List.range (max limit n + 1 - n)).map fun i =>
        .constructor (.array (n + i))
          (Pattern.expandLengthsList limit subpatterns
            ++ List.replicate (n + i - subpatterns.length) .wildcard))
  | .constructor head subpatterns =>
      .constructor head (Pattern.expandLengthsList limit subpatterns)

def Pattern.expandLengthsList (limit : Nat) : List Pattern → List Pattern
  | [] => []
  | p :: ps => Pattern.expandLengths limit p :: Pattern.expandLengthsList limit ps

end

theorem Pattern.expandLengthsList_eq_map (limit : Nat) (ps : List Pattern) :
    Pattern.expandLengthsList limit ps = ps.map (Pattern.expandLengths limit) := by
  induction ps with
  | nil => rfl
  | cons p ps ih => simp [Pattern.expandLengthsList, ih]

@[simp] theorem Pattern.expandLengthsList_length (limit : Nat) (ps : List Pattern) :
    (Pattern.expandLengthsList limit ps).length = ps.length := by
  simp [Pattern.expandLengthsList_eq_map]

/-- Every constructor but `arrayRest` is rewritten by recursing into its
sub-patterns and nothing else. -/
theorem Pattern.expandLengths_constructor (limit : Nat) (head : Constructor)
    (subpatterns : List Pattern) (hrest : ∀ n, head ≠ .arrayRest n) :
    Pattern.expandLengths limit (.constructor head subpatterns)
      = .constructor head (Pattern.expandLengthsList limit subpatterns) := by
  cases head with
  | arrayRest n => exact absurd rfl (hrest n)
  | _ => rfl

/-- A fixed-length array pattern matches only an array of that exact length. -/
theorem Pattern.matches_array_inv {k : Nat} {subpatterns : List Pattern} {w : Value}
    (h : Pattern.matches (.constructor (.array k) subpatterns) w = true) :
    ∃ vs, w = .array vs ∧ vs.length = k ∧ Pattern.matchesAll subpatterns vs = true := by
  have hrest : ∀ n, Constructor.array k ≠ .arrayRest n := by intro n; simp
  rw [Pattern.matches_constructor _ _ _ hrest] at h
  cases w with
  | array vs =>
    obtain ⟨hc, hall⟩ := Bool.and_eq_true .. |>.mp h
    simp only [Value.constructorOf, decide_eq_true_eq, Option.some.injEq] at hc
    cases hc
    exact ⟨vs, rfl, rfl, hall⟩
  | variant c j us => simp [Value.constructorOf] at h
  | single us => simp [Value.constructorOf] at h
  | bool b => simp [Value.constructorOf] at h
  | lit str => simp [Value.constructorOf] at h
  | opaqueValue => simp [Value.constructorOf] at h

/-- Soundness for a list of sub-patterns, given it holds for each. -/
theorem Pattern.matchesAll_expandLengths (limit : Nat) :
    ∀ (subpatterns : List Pattern) (ws : List Value),
      (∀ q ∈ subpatterns, ∀ w, Pattern.matches (q.expandLengths limit) w = true →
        Pattern.matches q w = true) →
      Pattern.matchesAll (Pattern.expandLengthsList limit subpatterns) ws = true →
      Pattern.matchesAll subpatterns ws = true := by
  intro subpatterns
  induction subpatterns with
  | nil => intro ws _ _; simp
  | cons p ps ih =>
    intro ws hsound h
    cases ws with
    | nil => exact absurd h (by simp [Pattern.expandLengthsList])
    | cons w ws =>
      have h' : (Pattern.matches (p.expandLengths limit) w &&
          Pattern.matchesAll (Pattern.expandLengthsList limit ps) ws) = true := h
      obtain ⟨h₁, h₂⟩ := Bool.and_eq_true .. |>.mp h'
      show (Pattern.matches p w && Pattern.matchesAll ps ws) = true
      rw [hsound p (by simp) w h₁, ih ws (fun q hq => hsound q (by simp [hq])) h₂]
      rfl

/-- The non-`arrayRest` half of `expandLengths_sound`: every other
constructor is rewritten by recursing into its sub-patterns and nothing else,
so soundness is exactly soundness of the sub-patterns. Factored out because
otherwise it is five identical case branches. -/
private theorem expandLengths_sound_constructor {limit : Nat} {head : Constructor}
    {subpatterns : List Pattern} {w : Value}
    (hrest : ∀ n, head ≠ .arrayRest n)
    (hsub : ∀ q ∈ subpatterns, ∀ u,
      Pattern.matches (q.expandLengths limit) u = true → Pattern.matches q u = true)
    (h : Pattern.matches ((Pattern.constructor head subpatterns).expandLengths limit) w = true) :
    Pattern.matches (.constructor head subpatterns) w = true := by
  rw [Pattern.expandLengths_constructor limit _ subpatterns hrest,
      Pattern.matches_constructor _ _ _ hrest] at h
  rw [Pattern.matches_constructor _ _ _ hrest]
  obtain ⟨hc, hall⟩ := Bool.and_eq_true .. |>.mp h
  rw [hc, Pattern.matchesAll_expandLengths limit subpatterns _ hsub hall]
  simp

/-- **`expand_lengths` is sound**: whatever the rewritten pattern matches, the
original matches. -/
theorem Pattern.expandLengths_sound (limit : Nat) :
    ∀ (p : Pattern) (w : Value), Pattern.LoweredArrays p →
      Pattern.matches (p.expandLengths limit) w = true → Pattern.matches p w = true := by
  intro p
  induction p using Pattern.ind' with
  | wildcard => intro w _ _; simp
  | or alternatives ihs =>
    intro w hlow h
    have hany : Pattern.matchesAny (Pattern.expandLengthsList limit alternatives) w = true := h
    obtain ⟨q', hq', hqm⟩ := Pattern.matchesAny_iff.mp hany
    -- `expandLengthsList` is a pointwise map, so `q'` comes from some `q`.
    rw [Pattern.expandLengthsList_eq_map] at hq'
    obtain ⟨q, hq, rfl⟩ := List.mem_map.mp hq'
    show Pattern.matchesAny alternatives w = true
    exact Pattern.matchesAny_iff.mpr
      ⟨q, hq, ihs q hq w (Pattern.LoweredArraysList.mem hlow hq) hqm⟩
  | constructor head subpatterns ihs =>
    intro w hlow h
    have hsub : ∀ q ∈ subpatterns, ∀ u,
        Pattern.matches (q.expandLengths limit) u = true → Pattern.matches q u = true := by
      intro q hq u
      refine ihs q hq u ?_
      cases head <;>
        first
          | exact Pattern.LoweredArraysList.mem hlow.2 hq
          | exact Pattern.LoweredArraysList.mem hlow hq
    cases head with
    | arrayRest n =>
      -- The rewritten pattern is a disjunction of fixed lengths.
      have hlen : subpatterns.length = n := hlow.1
      have hany : Pattern.matchesAny
          ((List.range (max limit n + 1 - n)).map fun i =>
            Pattern.constructor (.array (n + i))
              (Pattern.expandLengthsList limit subpatterns
                ++ List.replicate (n + i - subpatterns.length) .wildcard)) w = true := h
      obtain ⟨q, hq, hqm⟩ := Pattern.matchesAny_iff.mp hany
      obtain ⟨i, _, rfl⟩ := List.mem_map.mp hq
      obtain ⟨vs, rfl, hveq, hall⟩ := Pattern.matches_array_inv hqm
      show (decide (n ≤ vs.length) && Pattern.matchesAll subpatterns (vs.take n)) = true
      -- Split the padded sub-pattern list at `n` and keep the first half.
      have hsplit : Pattern.matchesAll
          (Pattern.expandLengthsList limit subpatterns) (vs.take n) = true := by
        have hlenEq : (Pattern.expandLengthsList limit subpatterns).length
            = (vs.take n).length := by
          simp only [Pattern.expandLengthsList_length, List.length_take, hlen]
          omega
        have hsp := Pattern.matchesAll_append
          (ps₁ := Pattern.expandLengthsList limit subpatterns)
          (ps₂ := List.replicate (n + i - subpatterns.length) Pattern.wildcard)
          (vs₁ := vs.take n) (vs₂ := vs.drop n) hlenEq
        rw [List.take_append_drop] at hsp
        rw [hsp] at hall
        exact (Bool.and_eq_true .. |>.mp hall).1
      rw [Pattern.matchesAll_expandLengths limit subpatterns (vs.take n) hsub hsplit]
      simp
      omega
    | _ => exact expandLengths_sound_constructor (by intro m; simp) hsub h

/-! ## `expand` -/

/-! ## `expand`

The rows `expand` produces from a one-column row: the pattern's top-level
disjuncts, flattened. It does *not* descend into constructors -- which is
exactly the defect recorded in `findings/README.md` §6. -/

mutual

/-- A pattern's top-level disjuncts. -/
def Pattern.topDisjuncts : Pattern → List Pattern
  | .wildcard => [.wildcard]
  | .constructor head subpatterns => [.constructor head subpatterns]
  | .or alternatives => Pattern.topDisjunctsList alternatives

def Pattern.topDisjunctsList : List Pattern → List Pattern
  | [] => []
  | p :: ps => Pattern.topDisjuncts p ++ Pattern.topDisjunctsList ps

end

theorem Pattern.mem_topDisjunctsList {q : Pattern} :
    ∀ {ps : List Pattern}, q ∈ Pattern.topDisjunctsList ps →
      ∃ p ∈ ps, q ∈ Pattern.topDisjuncts p := by
  intro ps
  induction ps with
  | nil => intro h; exact absurd h List.not_mem_nil
  | cons a as ih =>
    intro h
    rcases List.mem_append.mp h with h' | h'
    · exact ⟨a, by simp, h'⟩
    · obtain ⟨p, hp, hq⟩ := ih h'
      exact ⟨p, by simp [hp], hq⟩

/-- **`expand` is sound**: a disjunct matches only what the whole pattern
matches. -/
theorem Pattern.topDisjuncts_sound :
    ∀ (p q : Pattern) (w : Value), q ∈ p.topDisjuncts →
      Pattern.matches q w = true → Pattern.matches p w = true := by
  intro p
  induction p using Pattern.ind' with
  | wildcard =>
    intro q w hq hm
    simp only [Pattern.topDisjuncts, List.mem_singleton] at hq
    subst hq; exact hm
  | constructor head subpatterns _ =>
    intro q w hq hm
    simp only [Pattern.topDisjuncts, List.mem_singleton] at hq
    subst hq; exact hm
  | or alternatives ihs =>
    intro q w hq hm
    have hq' : q ∈ Pattern.topDisjunctsList alternatives := hq
    obtain ⟨a, ha, hqa⟩ := Pattern.mem_topDisjunctsList hq'
    show Pattern.matchesAny alternatives w = true
    exact Pattern.matchesAny_iff.mpr ⟨a, ha, ihs a ha q w hqa hm⟩

end Buri
