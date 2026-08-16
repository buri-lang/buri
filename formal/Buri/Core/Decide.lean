import Buri.Core.Typing

/-!
# Decidable equality, and Boolean mirrors of the side conditions

An *algorithm* has to decide things a declarative judgment may merely assert.
This file supplies the three decisions the checker needs:

* **type equality**. `Ty` is a nested inductive, so `deriving DecidableEq` has
  no handler for it; `Ty.beq` is written out and `Ty.beq_iff` is the proof. This
  is `Ty::eq` in Rust, which `#[derive(PartialEq)]` generates -- the same
  structural equality, since conformance is nominal (SPEC 5.12.1) and there is
  nothing up to which types are compared.
* **pattern well-formedness**, `Pattern.WellFormed`. Every theorem in
  `Patterns/` takes it as a hypothesis; the checker has to establish it.
* **`lower`'s array invariant**, `Pattern.LoweredArrays`, which
  `exhaustive_correct_unbounded` needs.

Each Boolean mirror comes with a *soundness* lemma only -- `= true` implies the
proposition. Completeness (the converse) is not needed and not proved: the
checker may reject, and `Sound.lean` says so explicitly.
-/

namespace Buri

/-! ## Type equality -/

mutual

def Ty.beq : Ty → Ty → Bool
  | .con c ts, .con c' ts' => c == c' && Ty.beqList ts ts'
  | .array e, .array e' => Ty.beq e e'
  | .tuple ts, .tuple ts' => Ty.beqList ts ts'
  | .fn ps r, .fn ps' r' => Ty.beqList ps ps' && Ty.beq r r'
  | .unit, .unit => true
  | .ctx k, .ctx k' => k == k'
  | .param n, .param n' => n == n'
  | _, _ => false

def Ty.beqList : List Ty → List Ty → Bool
  | [], [] => true
  | t :: ts, t' :: ts' => Ty.beq t t' && Ty.beqList ts ts'
  | _, _ => false

end

private theorem Ty.beqList_iff (ts : List Ty)
    (ih : ∀ t ∈ ts, ∀ t', Ty.beq t t' = true ↔ t = t') :
    ∀ ts', Ty.beqList ts ts' = true ↔ ts = ts' := by
  induction ts with
  | nil => intro ts'; cases ts' <;> simp [Ty.beqList]
  | cons t ts ihs =>
    intro ts'
    cases ts' with
    | nil => simp [Ty.beqList]
    | cons t' ts'' =>
      simp only [Ty.beqList, Bool.and_eq_true, ih t (by simp) t',
        ihs (fun q hq => ih q (by simp [hq])) ts'', List.cons.injEq]

theorem Ty.beq_iff : ∀ (a b : Ty), Ty.beq a b = true ↔ a = b := by
  intro a
  induction a using Ty.ind' with
  | con c args ih =>
    intro b; cases b <;>
      simp [Ty.beq, Ty.beqList_iff args ih]
  | array e ih => intro b; cases b <;> simp [Ty.beq, ih]
  | tuple ts ih => intro b; cases b <;> simp [Ty.beq, Ty.beqList_iff ts ih]
  | fn ps r ihp ihr =>
    intro b; cases b <;> simp [Ty.beq, Ty.beqList_iff ps ihp, ihr]
  | unit => intro b; cases b <;> simp [Ty.beq]
  | ctx k => intro b; cases b <;> simp [Ty.beq]
  | param n => intro b; cases b <;> simp [Ty.beq]

instance : DecidableEq Ty := fun a b => decidable_of_iff _ (Ty.beq_iff a b)

/-! ## Well-formedness, decided -/

mutual

def Pattern.wellFormedB (S : Signature) : Ty → Pattern → Bool
  | _, .wildcard => true
  | t, .or alternatives => Pattern.wellFormedAltsB S t alternatives
  | _, .constructor (.arrayRest _) _ => false
  | t, .constructor head subpatterns =>
      Pattern.wellFormedPrefixB S (head.fieldTypes S t) subpatterns

def Pattern.wellFormedPrefixB (S : Signature) : List Ty → List Pattern → Bool
  | _, [] => true
  | [], _ :: _ => false
  | fieldType :: fieldTypes, p :: ps =>
      Pattern.wellFormedB S fieldType p && Pattern.wellFormedPrefixB S fieldTypes ps

def Pattern.wellFormedAltsB (S : Signature) : Ty → List Pattern → Bool
  | _, [] => true
  | t, p :: ps => Pattern.wellFormedB S t p && Pattern.wellFormedAltsB S t ps

end

mutual

theorem Pattern.wellFormedB_sound {S : Signature} :
    ∀ (p : Pattern) (t : Ty), Pattern.wellFormedB S t p = true → Pattern.WellFormed S t p
  | .wildcard, _, _ => trivial
  | .or alternatives, t, h => Pattern.wellFormedAltsB_sound alternatives t h
  | .constructor (.arrayRest _) _, _, h => by simp [Pattern.wellFormedB] at h
  | .constructor (.variant c i) subpatterns, t, h =>
      Pattern.wellFormedPrefixB_sound _ subpatterns h
  | .constructor .single subpatterns, t, h => Pattern.wellFormedPrefixB_sound _ subpatterns h
  | .constructor (.bool b) subpatterns, t, h => Pattern.wellFormedPrefixB_sound _ subpatterns h
  | .constructor (.array n) subpatterns, t, h => Pattern.wellFormedPrefixB_sound _ subpatterns h
  | .constructor (.lit s) subpatterns, t, h => Pattern.wellFormedPrefixB_sound _ subpatterns h

theorem Pattern.wellFormedPrefixB_sound {S : Signature} :
    ∀ (fieldTypes : List Ty) (subpatterns : List Pattern),
      Pattern.wellFormedPrefixB S fieldTypes subpatterns = true →
      Pattern.WellFormedPrefix S fieldTypes subpatterns
  | _, [], _ => trivial
  | [], _ :: _, h => by simp [Pattern.wellFormedPrefixB] at h
  | ft :: fts, p :: ps, h =>
      ⟨Pattern.wellFormedB_sound p ft (by simp [Pattern.wellFormedPrefixB] at h; exact h.1),
       Pattern.wellFormedPrefixB_sound fts ps (by simp [Pattern.wellFormedPrefixB] at h; exact h.2)⟩

theorem Pattern.wellFormedAltsB_sound {S : Signature} :
    ∀ (alternatives : List Pattern) (t : Ty),
      Pattern.wellFormedAltsB S t alternatives = true →
      Pattern.WellFormedAlternatives S t alternatives
  | [], _, _ => trivial
  | p :: ps, t, h =>
      ⟨Pattern.wellFormedB_sound p t (by simp [Pattern.wellFormedAltsB] at h; exact h.1),
       Pattern.wellFormedAltsB_sound ps t (by simp [Pattern.wellFormedAltsB] at h; exact h.2)⟩

end

/-! ## `lower`'s array invariant, decided -/

mutual

def Pattern.loweredArraysB : Pattern → Bool
  | .wildcard => true
  | .or alternatives => Pattern.loweredArraysListB alternatives
  | .constructor (.array n) subpatterns =>
      (subpatterns.length == n) && Pattern.loweredArraysListB subpatterns
  | .constructor (.arrayRest n) subpatterns =>
      (subpatterns.length == n) && Pattern.loweredArraysListB subpatterns
  | .constructor _ subpatterns => Pattern.loweredArraysListB subpatterns

def Pattern.loweredArraysListB : List Pattern → Bool
  | [] => true
  | p :: ps => Pattern.loweredArraysB p && Pattern.loweredArraysListB ps

end

mutual

theorem Pattern.loweredArraysB_sound :
    ∀ (p : Pattern), Pattern.loweredArraysB p = true → Pattern.LoweredArrays p
  | .wildcard, _ => trivial
  | .or alternatives, h => Pattern.loweredArraysListB_sound alternatives h
  | .constructor (.array n) subpatterns, h =>
      ⟨by simp [Pattern.loweredArraysB] at h; exact h.1,
       Pattern.loweredArraysListB_sound subpatterns
         (by simp [Pattern.loweredArraysB] at h; exact h.2)⟩
  | .constructor (.arrayRest n) subpatterns, h =>
      ⟨by simp [Pattern.loweredArraysB] at h; exact h.1,
       Pattern.loweredArraysListB_sound subpatterns
         (by simp [Pattern.loweredArraysB] at h; exact h.2)⟩
  | .constructor (.variant c i) subpatterns, h => Pattern.loweredArraysListB_sound subpatterns h
  | .constructor .single subpatterns, h => Pattern.loweredArraysListB_sound subpatterns h
  | .constructor (.bool b) subpatterns, h => Pattern.loweredArraysListB_sound subpatterns h
  | .constructor (.lit s) subpatterns, h => Pattern.loweredArraysListB_sound subpatterns h

theorem Pattern.loweredArraysListB_sound :
    ∀ (ps : List Pattern), Pattern.loweredArraysListB ps = true → Pattern.LoweredArraysList ps
  | [], _ => trivial
  | p :: ps, h =>
      ⟨Pattern.loweredArraysB_sound p (by simp [Pattern.loweredArraysListB] at h; exact h.1),
       Pattern.loweredArraysListB_sound ps (by simp [Pattern.loweredArraysListB] at h; exact h.2)⟩

end

/-! ## Uniform binders, decided conservatively

The declarative rule asks that every alternative of an alternation bind the
*same* types. The checker asks for something stronger and type-free: that they
bind *nothing*. That accepts `.Some(true | false)` -- the case
`findings/README.md` 6 is about -- and rejects `.Some(x) | .Other(y)` even when
the two agree, which Buri would accept. It is a deliberate incompleteness, and
`Sound.lean` records it.
-/

mutual

def Pattern.uniformBindersB : Pattern → Bool
  | .wildcard => true
  | .constructor _ subpatterns => Pattern.uniformBindersListB subpatterns
  | .or alternatives =>
      Pattern.uniformBindersListB alternatives && Pattern.noBindersListB alternatives

def Pattern.uniformBindersListB : List Pattern → Bool
  | [] => true
  | p :: ps => Pattern.uniformBindersB p && Pattern.uniformBindersListB ps

def Pattern.noBindersListB : List Pattern → Bool
  | [] => true
  | p :: ps => (p.binderCount == 0) && Pattern.noBindersListB ps

end

/-- A well-formed pattern that counts no binders types none either. This is
`Pattern.binderTypes_length` read as an emptiness fact. -/
private theorem binderTypes_eq_nil {S : Signature} {t : Ty} {p : Pattern}
    (hwf : Pattern.WellFormed S t p) (h : p.binderCount = 0) :
    Pattern.binderTypes S t p = [] :=
  List.eq_nil_of_length_eq_zero (by rw [Pattern.binderTypes_length p t hwf, h])

private theorem uniformBindersAlts_nil {S : Signature} {t : Ty} :
    ∀ (alternatives : List Pattern),
      (∀ p ∈ alternatives, Pattern.WellFormed S t p) →
      (∀ p ∈ alternatives, Pattern.UniformBinders S t p) →
      (∀ p ∈ alternatives, p.binderCount = 0) →
      Pattern.UniformBindersAlts S t [] alternatives := by
  intro alternatives
  induction alternatives with
  | nil => intro _ _ _; trivial
  | cons a as ih =>
    intro hwf huni hcount
    exact ⟨⟨huni a (by simp), binderTypes_eq_nil (hwf a (by simp)) (hcount a (by simp))⟩,
      ih (fun q hq => hwf q (by simp [hq])) (fun q hq => huni q (by simp [hq]))
        (fun q hq => hcount q (by simp [hq]))⟩

private theorem noBindersListB_mem :
    ∀ {alternatives : List Pattern}, Pattern.noBindersListB alternatives = true →
      ∀ p ∈ alternatives, p.binderCount = 0 := by
  intro alternatives
  induction alternatives with
  | nil => intro _ p hp; exact absurd hp List.not_mem_nil
  | cons a as ih =>
    intro h p hp
    simp only [Pattern.noBindersListB, Bool.and_eq_true, beq_iff_eq] at h
    rcases List.mem_cons.mp hp with rfl | hp'
    · exact h.1
    · exact ih h.2 p hp'

private theorem uniformBindersListB_mem :
    ∀ {alternatives : List Pattern}, Pattern.uniformBindersListB alternatives = true →
      ∀ p ∈ alternatives, Pattern.uniformBindersB p = true := by
  intro alternatives
  induction alternatives with
  | nil => intro _ p hp; exact absurd hp List.not_mem_nil
  | cons a as ih =>
    intro h p hp
    simp only [Pattern.uniformBindersListB, Bool.and_eq_true] at h
    rcases List.mem_cons.mp hp with rfl | hp'
    · exact h.1
    · exact ih h.2 p hp'

private theorem uniformBindersList_sound {S : Signature} :
    ∀ (fieldTypes : List Ty) (subpatterns : List Pattern),
      Pattern.WellFormedPrefix S fieldTypes subpatterns →
      Pattern.uniformBindersListB subpatterns = true →
      (∀ p ∈ subpatterns, ∀ t, Pattern.WellFormed S t p → Pattern.uniformBindersB p = true →
        Pattern.UniformBinders S t p) →
      Pattern.UniformBindersList S fieldTypes subpatterns := by
  intro fieldTypes
  induction fieldTypes with
  | nil =>
    intro subpatterns hwf _ _
    cases subpatterns with
    | nil => trivial
    | cons _ _ => exact absurd hwf (by simp [Pattern.WellFormedPrefix])
  | cons ft fts ih =>
    intro subpatterns hwf h hsub
    cases subpatterns with
    | nil => trivial
    | cons q qs =>
      simp only [Pattern.uniformBindersListB, Bool.and_eq_true] at h
      exact ⟨hsub q (by simp) ft hwf.1 h.1,
        ih qs hwf.2 h.2 (fun r hr => hsub r (by simp [hr]))⟩

/-- The Boolean check implies the declarative side condition. Not conversely --
see the note above. -/
theorem Pattern.uniformBindersB_sound {S : Signature} :
    ∀ (p : Pattern) (t : Ty), Pattern.WellFormed S t p → Pattern.uniformBindersB p = true →
      Pattern.UniformBinders S t p := by
  intro p
  induction p using Pattern.ind' with
  | wildcard => intro t _ _; trivial
  | constructor head subpatterns ih =>
    intro t hwf h
    cases head with
    | arrayRest n => exact absurd hwf (by simp [Pattern.WellFormed])
    | _ => exact uniformBindersList_sound _ subpatterns hwf h ih
  | or alternatives ih =>
    intro t hwf h
    simp only [Pattern.uniformBindersB, Bool.and_eq_true] at h
    have hwfa : ∀ q ∈ alternatives, Pattern.WellFormed S t q :=
      fun q hq => Pattern.WellFormedAlternatives.mem hwf hq
    have hcount := noBindersListB_mem h.2
    have huni : ∀ q ∈ alternatives, Pattern.UniformBinders S t q :=
      fun q hq => ih q hq t (hwfa q hq) (uniformBindersListB_mem h.1 q hq)
    have hhead : Pattern.binderTypesHead S t alternatives = [] := by
      cases alternatives with
      | nil => rfl
      | cons a as =>
        exact binderTypes_eq_nil (hwfa a (by simp)) (hcount a (by simp))
    show Pattern.UniformBindersAlts S t (Pattern.binderTypesHead S t alternatives) alternatives
    rw [hhead]
    exact uniformBindersAlts_nil alternatives hwfa huni hcount

end Buri
