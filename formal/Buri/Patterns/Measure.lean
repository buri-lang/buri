import Buri.Patterns.Matrix

/-!
# The termination measure

`Ctx::useful` (`exhaust.rs:293`) is not structurally recursive, and the reason
is the wildcard-with-complete-constructor-set branch (`exhaust.rs:328`): it
replaces a single `_` by `arity` fresh `_`s, so the *pattern vector grows*.
Every other branch shrinks something obvious, which is why this is the only
part that needs an argument.

The measure is lexicographic:

    (matrix nodeCount + vector nodeCount,  vector length)

where a "node" is a constructor node -- wildcards count zero. Then:

* **`constructor` head.** The head node is consumed and replaced by its
  sub-patterns, so the first component drops by at least one.
* **`wildcard` head, constructor set incomplete.** `defaultMatrix` only drops rows and
  columns, so the first component cannot rise; the vector loses a column, so
  the second drops.
* **`wildcard` head, constructor set complete.** The vector's node count is
  unchanged -- `_` and `arity` copies of `_` both count zero -- and the vector
  *length* may rise. What saves it is that completeness forces some row of the
  matrix to be headed by the very constructor being specialised on, and
  `specialize` peels that row's head off. So the first component drops.

That last bullet is the whole content of `Matrix.nodeCount_specialize_lt`, and it is
why `isComplete` has to be a hypothesis rather than a convenience: the
algorithm terminates *because* it only expands wildcards when the matrix
already accounts for every constructor.
-/

namespace Buri

mutual

def Pattern.nodeCount : Pattern → Nat
  | .wildcard => 0
  | .constructor _ subpatterns => 1 + Pattern.nodeCountList subpatterns
  | .or alternatives => 1 + Pattern.nodeCountList alternatives

def Pattern.nodeCountList : List Pattern → Nat
  | [] => 0
  | p :: ps => Pattern.nodeCount p + Pattern.nodeCountList ps

end

/-- A row's node count. Definitionally `Pattern.nodeCountList`, so the two share every
lemma. -/
def Row.nodeCount (r : Row) : Nat := Pattern.nodeCountList r

def Matrix.nodeCount : List Row → Nat
  | [] => 0
  | r :: P => Row.nodeCount r + Matrix.nodeCount P

@[simp] theorem Row.nodeCount_nil : Row.nodeCount [] = 0 := rfl

@[simp] theorem Row.nodeCount_cons (p : Pattern) (r : Row) :
    Row.nodeCount (p :: r) = p.nodeCount + Row.nodeCount r := rfl

@[simp] theorem Pattern.nodeCount_wildcard : Pattern.nodeCount .wildcard = 0 := rfl

/-- The head node of a `constructor` pattern, made explicit: specialising a row
headed by `constructor c subpatterns` yields at most the sub-patterns' nodeCount, which is one
fewer than the head cost. -/
@[simp] theorem Pattern.nodeCount_constructor (c : Constructor) (subpatterns : List Pattern) :
    Pattern.nodeCount (.constructor c subpatterns) = 1 + Row.nodeCount subpatterns := rfl

@[simp] theorem Row.nodeCount_append (a b : Row) :
    Row.nodeCount (a ++ b) = Row.nodeCount a + Row.nodeCount b := by
  induction a with
  | nil => simp
  | cons x xs ih => simp [ih, Nat.add_assoc]

@[simp] theorem Row.nodeCount_replicate_wildcard (n : Nat) :
    Row.nodeCount (List.replicate n Pattern.wildcard) = 0 := by
  induction n with
  | zero => simp
  | succ k ih => simp [List.replicate, ih]

theorem Row.nodeCount_take_le (n : Nat) (r : Row) : Row.nodeCount (r.take n) ≤ Row.nodeCount r := by
  induction r generalizing n with
  | nil => simp
  | cons x xs ih =>
    cases n with
    | zero => simp
    | succ k =>
      have := ih k
      simp only [List.take_succ_cons, Row.nodeCount_cons]
      omega

@[simp] theorem Matrix.nodeCount_nil : Matrix.nodeCount [] = 0 := rfl

@[simp] theorem Matrix.nodeCount_cons (r : Row) (P : List Row) :
    Matrix.nodeCount (r :: P) = Row.nodeCount r + Matrix.nodeCount P := rfl

/-! ## Lifting a per-row bound to the matrix -/

@[simp] theorem Matrix.nodeCount_append (A B : List Row) :
    Matrix.nodeCount (A ++ B) = Matrix.nodeCount A + Matrix.nodeCount B := by
  induction A with
  | nil => simp
  | cons r A ih => simp [ih, Nat.add_assoc]

theorem Matrix.nodeCount_filterMap_le {f : Row → Option Row} (P : List Row)
    (h : ∀ r r', r ∈ P → f r = some r' → Row.nodeCount r' ≤ Row.nodeCount r) :
    Matrix.nodeCount (P.filterMap f) ≤ Matrix.nodeCount P := by
  induction P with
  | nil => simp
  | cons r P ih =>
    have ih' := ih fun a b ha hb => h a b (by simp [ha]) hb
    rw [List.filterMap_cons]
    cases hf : f r with
    | none => simp only [Matrix.nodeCount_cons]; omega
    | some r' =>
      have := h r r' (by simp) hf
      simp only [Matrix.nodeCount_cons]; omega

/-- The strict version, and the one the `complete` branch needs. If some row of
`P` is headed by a constructor and survives, the total node count drops.

Proved by splitting `P` at the witness row rather than by induction: the
induction version has to keep re-deriving the bound for the untouched prefix,
and the split makes the arithmetic a single `omega`. -/
theorem Matrix.nodeCount_filterMap_lt {f : Row → Option Row} (P : List Row) (r₀ : Row)
    (hmem : r₀ ∈ P)
    (hle : ∀ r r', f r = some r' → Row.nodeCount r' ≤ Row.nodeCount r)
    (hlt : ∀ r', f r₀ = some r' → Row.nodeCount r' < Row.nodeCount r₀)
    (hnone : f r₀ = none → 0 < Row.nodeCount r₀) :
    Matrix.nodeCount (P.filterMap f) < Matrix.nodeCount P := by
  obtain ⟨A, B, rfl⟩ := List.append_of_mem hmem
  have hA : Matrix.nodeCount (A.filterMap f) ≤ Matrix.nodeCount A :=
    Matrix.nodeCount_filterMap_le A fun a b _ hb => hle a b hb
  have hB : Matrix.nodeCount (B.filterMap f) ≤ Matrix.nodeCount B :=
    Matrix.nodeCount_filterMap_le B fun a b _ hb => hle a b hb
  rw [List.filterMap_append, List.filterMap_cons]
  cases hf : f r₀ with
  | none =>
    have := hnone hf
    simp only [Matrix.nodeCount_append, Matrix.nodeCount_cons]
    omega
  | some r' =>
    have := hlt r' hf
    simp only [Matrix.nodeCount_append, Matrix.nodeCount_cons]
    omega

/-! ## `specialize` and `defaultMatrix` never grow the matrix -/

theorem specializeRow_nodes_le {target : Constructor} {a : Nat} {r r' : Row}
    (h : specializeRow target a r = some r') : Row.nodeCount r' ≤ Row.nodeCount r := by
  match r with
  | [] => simp [specializeRow] at h
  | .wildcard :: rest => cases h; simp
  | .constructor c subpatterns :: rest =>
    simp only [specializeRow] at h
    split at h
    · cases h
      have := Row.nodeCount_take_le a (subpatterns ++ List.replicate a Pattern.wildcard)
      simp only [Row.nodeCount_append, Row.nodeCount_replicate_wildcard, Nat.add_zero] at this
      simp [Pattern.nodeCount_constructor]
      omega
    · simp at h
  | .or _ :: _ => simp [specializeRow] at h

theorem specializeRow_nodes_lt {target : Constructor} {a : Nat} {c : Constructor} {subpatterns : List Pattern}
    {rest r' : Row} (h : specializeRow target a (.constructor c subpatterns :: rest) = some r') :
    Row.nodeCount r' < Row.nodeCount (.constructor c subpatterns :: rest) := by
  simp only [specializeRow] at h
  split at h
  · cases h
    have := Row.nodeCount_take_le a (subpatterns ++ List.replicate a Pattern.wildcard)
    simp only [Row.nodeCount_append, Row.nodeCount_replicate_wildcard, Nat.add_zero] at this
    simp [Pattern.nodeCount_constructor]
    omega
  · simp at h

theorem Matrix.nodeCount_specialize_le (P : List Row) (target : Constructor) (a : Nat) :
    Matrix.nodeCount (specialize P target a) ≤ Matrix.nodeCount P :=
  Matrix.nodeCount_filterMap_le P fun _ _ _ hb => specializeRow_nodes_le hb

theorem Matrix.nodeCount_defaultMatrix_le (P : List Row) :
    Matrix.nodeCount (defaultMatrix P) ≤ Matrix.nodeCount P := by
  refine Matrix.nodeCount_filterMap_le P fun r r' _ hb => ?_
  match r with
  | .wildcard :: rest => cases hb; simp
  | [] => simp [defaultRow] at hb
  | .constructor _ _ :: _ => simp [defaultRow] at hb
  | .or _ :: _ => simp [defaultRow] at hb

/-- Membership in `headConstructors` is exactly "some row is headed by this
constructor". -/
theorem mem_headCtors {P : List Row} {c : Constructor} (h : c ∈ headConstructors P) :
    ∃ subpatterns rest, (Pattern.constructor c subpatterns :: rest) ∈ P := by
  induction P with
  | nil => simp [headConstructors] at h
  | cons r P ih =>
    match hr : r with
    | .constructor c' subpatterns :: rest =>
      simp only [headConstructors, List.filterMap_cons] at h
      rcases List.mem_cons.mp h with rfl | h'
      · exact ⟨subpatterns, rest, by simp⟩
      · obtain ⟨s, t, ht⟩ := ih h'
        exact ⟨s, t, by simp [ht]⟩
    | [] =>
      simp only [headConstructors, List.filterMap_cons] at h
      obtain ⟨s, t, ht⟩ := ih h
      exact ⟨s, t, by simp [ht]⟩
    | .wildcard :: rest =>
      simp only [headConstructors, List.filterMap_cons] at h
      obtain ⟨s, t, ht⟩ := ih h
      exact ⟨s, t, by simp [ht]⟩
    | .or _ :: rest =>
      simp only [headConstructors, List.filterMap_cons] at h
      obtain ⟨s, t, ht⟩ := ih h
      exact ⟨s, t, by simp [ht]⟩

/-- **The termination lemma.** When the matrix already accounts for `target`,
specialising on `target` strictly shrinks it. -/
theorem Matrix.nodeCount_specialize_lt (P : List Row) (target : Constructor) (a : Nat)
    (h : target ∈ headConstructors P) :
    Matrix.nodeCount (specialize P target a) < Matrix.nodeCount P := by
  obtain ⟨subpatterns, rest, hmem⟩ := mem_headCtors h
  refine Matrix.nodeCount_filterMap_lt P _ hmem
    (fun _ _ hb => specializeRow_nodes_le hb)
    (fun r' hb => specializeRow_nodes_lt hb)
    (fun _ => ?_)
  -- A constructor-headed row always has at least one node, so even if it were
  -- dropped outright the count would fall.
  simp only [Row.nodeCount_cons, Pattern.nodeCount_constructor]
  omega

end Buri
