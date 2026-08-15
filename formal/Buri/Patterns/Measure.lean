import Buri.Patterns.Matrix

/-!
# The termination measure

`Ctx::useful` (`exhaust.rs:293`) is not structurally recursive, and the reason
is the wildcard-with-complete-constructor-set branch (`exhaust.rs:328`): it
replaces a single `_` by `arity` fresh `_`s, so the *pattern vector grows*.
Every other branch shrinks something obvious, which is why this is the only
part that needs an argument.

The measure is lexicographic:

    (matrix nodes + vector nodes,  vector length)

where a "node" is a constructor node -- wildcards count zero. Then:

* **`ctor` head.** The head node is consumed and replaced by its
  sub-patterns, so the first component drops by at least one.
* **`wild` head, constructor set incomplete.** `defaultMat` only drops rows and
  columns, so the first component cannot rise; the vector loses a column, so
  the second drops.
* **`wild` head, constructor set complete.** The vector's node count is
  unchanged -- `_` and `arity` copies of `_` both count zero -- and the vector
  *length* may rise. What saves it is that completeness forces some row of the
  matrix to be headed by the very constructor being specialised on, and
  `specialize` peels that row's head off. So the first component drops.

That last bullet is the whole content of `Mat.nodes_specialize_lt`, and it is
why `isComplete` has to be a hypothesis rather than a convenience: the
algorithm terminates *because* it only expands wildcards when the matrix
already accounts for every constructor.
-/

namespace Buri

mutual

def Pat.nodes : Pat → Nat
  | .wild => 0
  | .ctor _ subs => 1 + Pat.nodesList subs
  | .or alts => 1 + Pat.nodesList alts

def Pat.nodesList : List Pat → Nat
  | [] => 0
  | p :: ps => Pat.nodes p + Pat.nodesList ps

end

/-- A row's node count. Definitionally `Pat.nodesList`, so the two share every
lemma. -/
def Row.nodes (r : Row) : Nat := Pat.nodesList r

def Mat.nodes : List Row → Nat
  | [] => 0
  | r :: P => Row.nodes r + Mat.nodes P

@[simp] theorem Row.nodes_nil : Row.nodes [] = 0 := rfl

@[simp] theorem Row.nodes_cons (p : Pat) (r : Row) :
    Row.nodes (p :: r) = p.nodes + Row.nodes r := rfl

@[simp] theorem Pat.nodes_wild : Pat.nodes .wild = 0 := rfl

/-- The head node of a `ctor` pattern, made explicit: specialising a row
headed by `ctor c subs` yields at most the sub-patterns' nodes, which is one
fewer than the head cost. -/
@[simp] theorem Pat.nodes_ctor (c : Ctor) (subs : List Pat) :
    Pat.nodes (.ctor c subs) = 1 + Row.nodes subs := rfl

@[simp] theorem Row.nodes_append (a b : Row) :
    Row.nodes (a ++ b) = Row.nodes a + Row.nodes b := by
  induction a with
  | nil => simp
  | cons x xs ih => simp [ih, Nat.add_assoc]

@[simp] theorem Row.nodes_replicate_wild (n : Nat) :
    Row.nodes (List.replicate n Pat.wild) = 0 := by
  induction n with
  | zero => simp
  | succ k ih => simp [List.replicate, ih]

theorem Row.nodes_take_le (n : Nat) (r : Row) : Row.nodes (r.take n) ≤ Row.nodes r := by
  induction r generalizing n with
  | nil => simp
  | cons x xs ih =>
    cases n with
    | zero => simp
    | succ k =>
      have := ih k
      simp only [List.take_succ_cons, Row.nodes_cons]
      omega

@[simp] theorem Mat.nodes_nil : Mat.nodes [] = 0 := rfl

@[simp] theorem Mat.nodes_cons (r : Row) (P : List Row) :
    Mat.nodes (r :: P) = Row.nodes r + Mat.nodes P := rfl

/-! ## Lifting a per-row bound to the matrix -/

@[simp] theorem Mat.nodes_append (A B : List Row) :
    Mat.nodes (A ++ B) = Mat.nodes A + Mat.nodes B := by
  induction A with
  | nil => simp
  | cons r A ih => simp [ih, Nat.add_assoc]

theorem Mat.nodes_filterMap_le {f : Row → Option Row} (P : List Row)
    (h : ∀ r r', r ∈ P → f r = some r' → Row.nodes r' ≤ Row.nodes r) :
    Mat.nodes (P.filterMap f) ≤ Mat.nodes P := by
  induction P with
  | nil => simp
  | cons r P ih =>
    have ih' := ih fun a b ha hb => h a b (by simp [ha]) hb
    rw [List.filterMap_cons]
    cases hf : f r with
    | none => simp only [Mat.nodes_cons]; omega
    | some r' =>
      have := h r r' (by simp) hf
      simp only [Mat.nodes_cons]; omega

/-- The strict version, and the one the `complete` branch needs. If some row of
`P` is headed by a constructor and survives, the total node count drops.

Proved by splitting `P` at the witness row rather than by induction: the
induction version has to keep re-deriving the bound for the untouched prefix,
and the split makes the arithmetic a single `omega`. -/
theorem Mat.nodes_filterMap_lt {f : Row → Option Row} (P : List Row) (r₀ : Row)
    (hmem : r₀ ∈ P)
    (hle : ∀ r r', f r = some r' → Row.nodes r' ≤ Row.nodes r)
    (hlt : ∀ r', f r₀ = some r' → Row.nodes r' < Row.nodes r₀)
    (hnone : f r₀ = none → 0 < Row.nodes r₀) :
    Mat.nodes (P.filterMap f) < Mat.nodes P := by
  obtain ⟨A, B, rfl⟩ := List.append_of_mem hmem
  have hA : Mat.nodes (A.filterMap f) ≤ Mat.nodes A :=
    Mat.nodes_filterMap_le A fun a b _ hb => hle a b hb
  have hB : Mat.nodes (B.filterMap f) ≤ Mat.nodes B :=
    Mat.nodes_filterMap_le B fun a b _ hb => hle a b hb
  rw [List.filterMap_append, List.filterMap_cons]
  cases hf : f r₀ with
  | none =>
    have := hnone hf
    simp only [Mat.nodes_append, Mat.nodes_cons]
    omega
  | some r' =>
    have := hlt r' hf
    simp only [Mat.nodes_append, Mat.nodes_cons]
    omega

/-! ## `specialize` and `defaultMat` never grow the matrix -/

theorem specializeRow_nodes_le {ct : Ctor} {a : Nat} {r r' : Row}
    (h : specializeRow ct a r = some r') : Row.nodes r' ≤ Row.nodes r := by
  match r with
  | [] => simp [specializeRow] at h
  | .wild :: rest => cases h; simp
  | .ctor c subs :: rest =>
    simp only [specializeRow] at h
    split at h
    · cases h
      have := Row.nodes_take_le a (subs ++ List.replicate a Pat.wild)
      simp only [Row.nodes_append, Row.nodes_replicate_wild, Nat.add_zero] at this
      simp [Pat.nodes_ctor]
      omega
    · simp at h
  | .or _ :: _ => simp [specializeRow] at h

theorem specializeRow_nodes_lt {ct : Ctor} {a : Nat} {c : Ctor} {subs : List Pat}
    {rest r' : Row} (h : specializeRow ct a (.ctor c subs :: rest) = some r') :
    Row.nodes r' < Row.nodes (.ctor c subs :: rest) := by
  simp only [specializeRow] at h
  split at h
  · cases h
    have := Row.nodes_take_le a (subs ++ List.replicate a Pat.wild)
    simp only [Row.nodes_append, Row.nodes_replicate_wild, Nat.add_zero] at this
    simp [Pat.nodes_ctor]
    omega
  · simp at h

theorem Mat.nodes_specialize_le (P : List Row) (ct : Ctor) (a : Nat) :
    Mat.nodes (specialize P ct a) ≤ Mat.nodes P :=
  Mat.nodes_filterMap_le P fun _ _ _ hb => specializeRow_nodes_le hb

theorem Mat.nodes_defaultMat_le (P : List Row) :
    Mat.nodes (defaultMat P) ≤ Mat.nodes P := by
  refine Mat.nodes_filterMap_le P fun r r' _ hb => ?_
  match r with
  | .wild :: rest => cases hb; simp
  | [] => simp [defaultRow] at hb
  | .ctor _ _ :: _ => simp [defaultRow] at hb
  | .or _ :: _ => simp [defaultRow] at hb

/-- Membership in `headCtors` is exactly "some row is headed by this
constructor". -/
theorem mem_headCtors {P : List Row} {c : Ctor} (h : c ∈ headCtors P) :
    ∃ subs rest, (Pat.ctor c subs :: rest) ∈ P := by
  induction P with
  | nil => simp [headCtors] at h
  | cons r P ih =>
    match hr : r with
    | .ctor c' subs :: rest =>
      simp only [headCtors, List.filterMap_cons] at h
      rcases List.mem_cons.mp h with rfl | h'
      · exact ⟨subs, rest, by simp⟩
      · obtain ⟨s, t, ht⟩ := ih h'
        exact ⟨s, t, by simp [ht]⟩
    | [] =>
      simp only [headCtors, List.filterMap_cons] at h
      obtain ⟨s, t, ht⟩ := ih h
      exact ⟨s, t, by simp [ht]⟩
    | .wild :: rest =>
      simp only [headCtors, List.filterMap_cons] at h
      obtain ⟨s, t, ht⟩ := ih h
      exact ⟨s, t, by simp [ht]⟩
    | .or _ :: rest =>
      simp only [headCtors, List.filterMap_cons] at h
      obtain ⟨s, t, ht⟩ := ih h
      exact ⟨s, t, by simp [ht]⟩

/-- **The termination lemma.** When the matrix already accounts for `ct`,
specialising on `ct` strictly shrinks it. -/
theorem Mat.nodes_specialize_lt (P : List Row) (ct : Ctor) (a : Nat)
    (h : ct ∈ headCtors P) :
    Mat.nodes (specialize P ct a) < Mat.nodes P := by
  obtain ⟨subs, rest, hmem⟩ := mem_headCtors h
  refine Mat.nodes_filterMap_lt P _ hmem
    (fun _ _ hb => specializeRow_nodes_le hb)
    (fun r' hb => specializeRow_nodes_lt hb)
    (fun _ => ?_)
  -- A constructor-headed row always has at least one node, so even if it were
  -- dropped outright the count would fall.
  simp only [Row.nodes_cons, Pat.nodes_ctor]
  omega

end Buri
