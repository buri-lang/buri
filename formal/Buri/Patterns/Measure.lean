import Buri.Patterns.Matrix

/-!
# The termination measure

`Ctx::useful` (`exhaustiveness.rs`) is not structurally recursive, and two
branches are the reason:

* the **wildcard-with-complete-constructor-set** branch replaces a single `_`
  by `arity` fresh `_`s, so the pattern vector *grows*; and
* `specialize` and `default_matrix` **distribute** over an or-headed row
  (`findings/README.md` 6), so the matrix gains rows.

The measure is lexicographic, `(matrix weight + vector weight, vector length)`,
where the weight of a *row* is the **product** of its columns' weights and

    weight(_)            = 1
    weight(c(p₁ .. pₙ))  = 1 + weight(p₁) * .. * weight(pₙ)
    weight(p₁ | .. | pₙ) = 1 + weight(p₁) + .. + weight(pₙ)

A product, not a sum, because distributing an alternation in the head column
*copies the rest of the row*: `(a | b) :: rest` becomes `a :: rest` and
`b :: rest`, and only a measure that multiplies through the tail can see that
`weight(a) + weight(b) < weight(a | b)` dominates the duplication. The `1 +` in
each constructor case is what makes every step strict rather than merely
non-increasing.

Then, branch by branch:

* **`constructor` head in the vector.** The head node is consumed and replaced
  by its sub-patterns, so the vector's weight drops from
  `(1 + ∏ weight(subs)) * w` to at most `∏ weight(subs) * w`; the matrix cannot
  grow (`Matrix.weight_specialize_le`).
* **`or` head in the vector.** One alternative replaces the alternation, and
  `weight(aᵢ) < 1 + Σ weight(aⱼ)`.
* **`wildcard` head, constructor set incomplete.** `defaultMatrix` cannot grow
  the matrix; the vector loses a column, so the second component drops.
* **`wildcard` head, constructor set complete.** The vector's weight is
  unchanged -- `_` and `arity` copies of `_` both weigh `1`, and `1` is the
  unit of the product. What saves it is that completeness forces some row of
  the matrix to be headed by something other than a wildcard, and `specialize`
  strictly shrinks every such row. So the first component drops.

That last bullet is `Matrix.weight_specialize_lt`, and it is why completeness
has to be a hypothesis rather than a convenience: **the algorithm terminates
because it only expands a wildcard when the matrix has already paid for the
expansion.**
-/

namespace Buri

mutual

/-- A pattern's weight. See the module docstring for why constructors multiply
and alternations add. -/
def Pattern.weight : Pattern → Nat
  | .wildcard => 1
  | .constructor _ subpatterns => 1 + Pattern.weightProd subpatterns
  | .or alternatives => 1 + Pattern.weightSum alternatives

def Pattern.weightProd : List Pattern → Nat
  | [] => 1
  | p :: ps => Pattern.weight p * Pattern.weightProd ps

def Pattern.weightSum : List Pattern → Nat
  | [] => 0
  | p :: ps => Pattern.weight p + Pattern.weightSum ps

end

theorem Pattern.weightProd_eq (ps : List Pattern) :
    Pattern.weightProd ps = (ps.map Pattern.weight).prod := by
  induction ps with
  | nil => rfl
  | cons p ps ih => simp [Pattern.weightProd, ih]

theorem Pattern.weightSum_eq (ps : List Pattern) :
    Pattern.weightSum ps = (ps.map Pattern.weight).sum := by
  induction ps with
  | nil => rfl
  | cons p ps ih => simp [Pattern.weightSum, ih]

/-! ## Arithmetic

Everything below is the fact that weights are at least `1`, plus the two
homomorphisms: a row's weight is multiplicative in `++`, a matrix's is additive.
-/

/-- A row weighs the product of its columns. -/
def Row.weight (r : Row) : Nat := Pattern.weightProd r

/-- A matrix weighs the sum of its rows. -/
def Matrix.weight (matrix : List Row) : Nat := (matrix.map Row.weight).sum

@[simp] theorem Pattern.weight_wildcard : Pattern.weight .wildcard = 1 := rfl

@[simp] theorem Pattern.weight_constructor (c : Constructor) (subpatterns : List Pattern) :
    Pattern.weight (.constructor c subpatterns) = 1 + Row.weight subpatterns := rfl

@[simp] theorem Pattern.weight_or (alternatives : List Pattern) :
    Pattern.weight (.or alternatives) = 1 + Pattern.weightSum alternatives := rfl

@[simp] theorem Row.weight_nil : Row.weight [] = 1 := rfl

@[simp] theorem Row.weight_cons (p : Pattern) (r : Row) :
    Row.weight (p :: r) = p.weight * Row.weight r := rfl

theorem Pattern.one_le_weight (p : Pattern) : 1 ≤ p.weight := by
  cases p <;> simp

theorem Row.one_le_weight (r : Row) : 1 ≤ Row.weight r := by
  induction r with
  | nil => simp
  | cons p ps ih =>
    have h := Pattern.one_le_weight p
    calc 1 = 1 * 1 := by simp
    _ ≤ p.weight * Row.weight ps := Nat.mul_le_mul h ih

@[simp] theorem Row.weight_append (a b : Row) :
    Row.weight (a ++ b) = Row.weight a * Row.weight b := by
  induction a with
  | nil => simp
  | cons p ps ih => simp [ih, Nat.mul_assoc]

@[simp] theorem Row.weight_replicate_wildcard (n : Nat) :
    Row.weight (List.replicate n Pattern.wildcard) = 1 := by
  induction n with
  | zero => simp
  | succ k ih => simp [List.replicate, ih]

theorem Row.weight_take_le (n : Nat) (r : Row) : Row.weight (r.take n) ≤ Row.weight r := by
  induction r generalizing n with
  | nil => simp
  | cons p ps ih =>
    cases n with
    | zero => simpa using Row.one_le_weight (p :: ps)
    | succ k =>
      simp only [List.take_succ_cons, Row.weight_cons]
      exact Nat.mul_le_mul_left _ (ih k)

/-- An alternative weighs no more than the alternation it belongs to. -/
theorem Pattern.weight_le_weightSum {p : Pattern} {ps : List Pattern} (h : p ∈ ps) :
    p.weight ≤ Pattern.weightSum ps := by
  induction ps with
  | nil => exact absurd h List.not_mem_nil
  | cons q qs ih =>
    rcases List.mem_cons.mp h with rfl | h'
    · simp only [Pattern.weightSum]; omega
    · have := ih h'
      simp only [Pattern.weightSum]
      omega

/-- The pad-and-truncate `specializeRow` performs never adds weight. -/
theorem Row.weight_pad_le (subpatterns : Row) (arity : Nat) :
    Row.weight ((subpatterns ++ List.replicate arity Pattern.wildcard).take arity)
      ≤ Row.weight subpatterns := by
  have h := Row.weight_take_le arity (subpatterns ++ List.replicate arity Pattern.wildcard)
  rwa [Row.weight_append, Row.weight_replicate_wildcard, Nat.mul_one] at h

@[simp] theorem Matrix.weight_nil : Matrix.weight [] = 0 := rfl

@[simp] theorem Matrix.weight_cons (r : Row) (matrix : List Row) :
    Matrix.weight (r :: matrix) = Row.weight r + Matrix.weight matrix := by
  simp [Matrix.weight]

@[simp] theorem Matrix.weight_singleton (r : Row) : Matrix.weight [r] = Row.weight r := by
  simp [Matrix.weight]

@[simp] theorem Matrix.weight_append (a b : List Row) :
    Matrix.weight (a ++ b) = Matrix.weight a + Matrix.weight b := by
  simp [Matrix.weight]

/-- The bound for a `flatMap`, with the per-element budget given by `w`. Both
matrix operations are `flatMap`s, and both the `≤` and the `<` lemmas below go
through this. -/
theorem Matrix.weight_flatMap_le {α : Type} (l : List α) (f : α → List Row) (w : α → Nat)
    (h : ∀ a ∈ l, Matrix.weight (f a) ≤ w a) :
    Matrix.weight (l.flatMap f) ≤ (l.map w).sum := by
  induction l with
  | nil => simp
  | cons a l ih =>
    have ha := h a (by simp)
    have ihl := ih fun b hb => h b (by simp [hb])
    simp only [List.flatMap_cons, List.map_cons, List.sum_cons, Matrix.weight_append]
    omega

/-! ## The two operations never grow the matrix

Both are proved by the recursion `specializeRow` and `defaultRow` themselves
use: the only interesting case is the or-headed row, where distribution
replaces one row of weight `(1 + Σ weight(aᵢ)) * weight(rest)` by rows totalling
at most `(Σ weight(aᵢ)) * weight(rest)`. The `1 +` is the slack.
-/

private theorem sum_mul_weight (l : List Pattern) (k : Nat) :
    (l.map fun p => p.weight * k).sum = (l.map Pattern.weight).sum * k := by
  induction l with
  | nil => simp
  | cons a as ih => simp [ih, Nat.add_mul]

/-- Distributing an alternation over a tail costs `weightSum * weight(rest)`,
which the alternation's own `1 +` more than pays for. -/
private theorem weight_distribute_le {alternatives : List Pattern} {rest : Row}
    {f : Pattern → List Row}
    (h : ∀ a ∈ alternatives, Matrix.weight (f a) ≤ Row.weight (a :: rest)) :
    Matrix.weight (alternatives.attach.flatMap fun a => f a.1)
      ≤ Pattern.weightSum alternatives * Row.weight rest := by
  refine Nat.le_trans (Matrix.weight_flatMap_le _ _ (fun a => Row.weight (a.1 :: rest))
    (fun a _ => h a.1 a.2)) ?_
  have hmap : (alternatives.attach.map fun a => Row.weight (a.1 :: rest))
      = alternatives.map fun p => p.weight * Row.weight rest :=
    List.attach_map_val (l := alternatives) (f := fun p => Row.weight (p :: rest))
  rw [hmap, sum_mul_weight, Pattern.weightSum_eq]
  exact Nat.le_refl _

theorem Row.weight_specializeRow_le (target : Constructor) (arity : Nat) (row : Row) :
    Matrix.weight (specializeRow target arity row) ≤ Row.weight row := by
  induction row using specializeRow.induct target with
  | case1 => simp
  | case2 rest => simp
  | case3 subpatterns rest =>
    have hpad := Nat.mul_le_mul_right (Row.weight rest) (Row.weight_pad_le subpatterns arity)
    have hstep : specializeRow target arity (Pattern.constructor target subpatterns :: rest)
        = [(subpatterns ++ List.replicate arity Pattern.wildcard).take arity ++ rest] := by
      rw [specializeRow]; simp
    rw [hstep]
    simp only [Matrix.weight_singleton, Row.weight_append, Row.weight_cons,
      Pattern.weight_constructor, Nat.add_mul, Nat.one_mul]
    omega
  | case4 c subpatterns rest hne => simp [hne]
  | case5 alternatives rest ih =>
    have hrest := Row.one_le_weight rest
    have hd := weight_distribute_le (alternatives := alternatives) (rest := rest)
      (f := fun a => specializeRow target arity (a :: rest)) (fun a ha => ih ⟨a, ha⟩)
    simp only [specializeRow, Row.weight_cons, Pattern.weight_or, Nat.add_mul, Nat.one_mul]
    omega

/-- A row whose head is *not* a wildcard strictly shrinks under `specialize` --
including the empty row, which contributes nothing. This is the whole content
of the termination argument for the wildcard-complete branch. -/
theorem Row.weight_specializeRow_lt (target : Constructor) (arity : Nat) (row : Row)
    (hhead : ∀ rest, row ≠ Pattern.wildcard :: rest) :
    Matrix.weight (specializeRow target arity row) < Row.weight row := by
  match row with
  | [] => simp
  | .wildcard :: rest => exact absurd rfl (hhead rest)
  | .constructor c subpatterns :: rest =>
    have hrest := Row.one_le_weight rest
    have hpad := Nat.mul_le_mul_right (Row.weight rest) (Row.weight_pad_le subpatterns arity)
    rw [specializeRow]
    split <;>
      simp only [Matrix.weight_singleton, Matrix.weight_nil, Row.weight_append, Row.weight_cons,
        Pattern.weight_constructor, Nat.add_mul, Nat.one_mul] <;>
      omega
  | .or alternatives :: rest =>
    have hrest := Row.one_le_weight rest
    have hd := weight_distribute_le (alternatives := alternatives) (rest := rest)
      (f := fun a => specializeRow target arity (a :: rest))
      (fun a _ => Row.weight_specializeRow_le target arity (a :: rest))
    simp only [specializeRow, Row.weight_cons, Pattern.weight_or, Nat.add_mul, Nat.one_mul]
    omega

theorem Row.weight_defaultRow_le (row : Row) :
    Matrix.weight (defaultRow row) ≤ Row.weight row := by
  induction row using defaultRow.induct with
  | case1 => simp
  | case2 rest => simp
  | case3 c subpatterns rest => simp
  | case4 alternatives rest ih =>
    have hrest := Row.one_le_weight rest
    have hd := weight_distribute_le (alternatives := alternatives) (rest := rest)
      (f := fun a => defaultRow (a :: rest)) (fun a ha => ih ⟨a, ha⟩)
    simp only [defaultRow, Row.weight_cons, Pattern.weight_or, Nat.add_mul, Nat.one_mul]
    omega

/-! ## Lifting to matrices -/

theorem Matrix.weight_specialize_le (matrix : List Row) (target : Constructor) (arity : Nat) :
    Matrix.weight (specialize matrix target arity) ≤ Matrix.weight matrix := by
  refine Nat.le_trans (Matrix.weight_flatMap_le _ _ Row.weight
    (fun r _ => Row.weight_specializeRow_le target arity r)) ?_
  simp [Matrix.weight]

theorem Matrix.weight_defaultMatrix_le (matrix : List Row) :
    Matrix.weight (defaultMatrix matrix) ≤ Matrix.weight matrix := by
  refine Nat.le_trans (Matrix.weight_flatMap_le _ _ Row.weight
    (fun r _ => Row.weight_defaultRow_le r)) ?_
  simp [Matrix.weight]

/-- Membership in `headConstructors` means some row is headed by something
other than a wildcard. That is all the termination argument needs -- it does
not care *which* row, only that one of them pays. -/
theorem mem_headConstructors_head {matrix : List Row} {c : Constructor}
    (h : c ∈ headConstructors matrix) :
    ∃ row ∈ matrix, ∀ rest, row ≠ Pattern.wildcard :: rest := by
  obtain ⟨row, hrow, hc⟩ := List.mem_flatMap.mp h
  refine ⟨row, hrow, ?_⟩
  rintro rest rfl
  simp at hc

/-- **The termination lemma.** When the matrix already accounts for `target`,
specialising on `target` strictly shrinks it. -/
theorem Matrix.weight_specialize_lt (matrix : List Row) (target : Constructor) (arity : Nat)
    (h : target ∈ headConstructors matrix) :
    Matrix.weight (specialize matrix target arity) < Matrix.weight matrix := by
  obtain ⟨row₀, hmem, hhead⟩ := mem_headConstructors_head h
  obtain ⟨A, B, rfl⟩ := List.append_of_mem hmem
  have hA := Matrix.weight_specialize_le A target arity
  have hB := Matrix.weight_specialize_le B target arity
  have h₀ := Row.weight_specializeRow_lt target arity row₀ hhead
  simp only [_root_.Buri.specialize, List.flatMap_append, List.flatMap_cons,
    Matrix.weight_append, Matrix.weight_cons] at *
  omega

end Buri
