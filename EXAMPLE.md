# The Kantorovich–Rubinstein Banach space

This file equips the space `M(Ω)` of finite signed Borel measures on a
compact metric space `Ω` with the **Kantorovich–Rubinstein norm**, making
it a real Banach space.  The probability simplex `ProbDist Ω` embeds into
this space as a closed convex subset, and the norm of `μ − ν` for two
probability measures coincides with the existing `krDist μ ν`.

This is the substrate on which the abstract supergradient theorem of
`Econlib.Math.Analysis.Supergradient` is later applied to derive the
"prices for moments" results in the persuasion duality.

## Definition of the KR norm

Following Hanin (1992), we use the basepoint formulation: fix `ω₀ ∈ Ω` and
define
```
  ‖μ‖_KR  :=  |μ(Ω)|  +  sup { ∫ p dμ : p ∈ Lip₁(Ω), p(ω₀) = 0 }.
```
The supremum on the right is finite because every 1-Lipschitz function
vanishing at `ω₀` is bounded in absolute value by `diam Ω` on the compact
space `Ω`, and `μ` has finite total variation.  Different choices of
basepoint give equivalent norms (and the same norm restricted to
zero-total-mass signed measures).

## Main constructions

* `KRSignedMeasure Ω` — wrapper structure over `MeasureTheory.SignedMeasure
  Ω`, used to install the KR-norm instances without colliding with any
  total-variation norm Mathlib may attach to signed measures.
* `KRSignedMeasure.krNorm` — the KR norm.
* `NormedAddCommGroup (KRSignedMeasure Ω)` — KR-normed group instance.
* `NormedSpace ℝ (KRSignedMeasure Ω)` — real vector-space structure.
* `CompleteSpace (KRSignedMeasure Ω)` — Banach space (Hanin 1992).
* `KRSignedMeasure.ofProbDist : ProbDist Ω → KRSignedMeasure Ω` — embedding
  of the probability simplex.
* `KRSignedMeasure.norm_ofProbDist_sub` — `‖ofProbDist μ − ofProbDist ν‖
  = krDist μ ν`, recovering the project's existing KR distance.

## References

* L. G. Hanin, *Kantorovich–Rubinstein norm and its application in the
  theory of Lipschitz spaces*, Proc. AMS 115 (1992), 345–352.
* V. I. Bogachev, *Measure Theory*, Vol. II, Exercise 8.10.143.
