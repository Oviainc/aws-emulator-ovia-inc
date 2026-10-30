Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dc8acfad318c0cd148284152a9b68532fd3f8c27

# CreateWorkload stores its Lenses list without the lens-identity de-duplication AssociateLenses applies, so Workload$Lenses reports one lens twice while the workload holds one review

_Filed: 2026-10-28T15:56:34Z_
_Operation: CreateWorkload | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dc8acfad318c0cd148284152a9b68532fd3f8c27`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateWorkload` assigns the request's `Lenses` list to the stored record verbatim. `WorkloadLenses` carries no `@uniqueItems`, and a lens is the same lens under several spellings (a bare official alias, the official lens ARN, a version-pinned ARN) — the crate's own identity rule, `crate::state::lens_alias`.

Consequence: a create naming one lens twice (literally twice, or once as the alias and once as the ARN) stores TWO entries for it. `Workload$Lenses` is projected straight off that list, so `GetWorkload`, `UpdateWorkload` and `ListWorkloads` report the lens twice, while everything derived through `lens_reviews_of` — `ListLensReviews`, the risk roll-up, `GetLensReview` — reports exactly ONE review of it. The same workload therefore reports two lenses and one lens review at the same time.

The crate already owns the de-duplication this path skips, and its sibling attach operation applies it before storing: `AssociateLenses` keeps each lens once by `lens_alias` and its stored list is pinned by a test row to hold one entry for a twice-named lens. `CreateWorkload` even builds the very same alias set, but uses it ONLY to keep an applied review template's lenses from colliding with the request's — never to de-duplicate the request's own list against itself.

Secondary symptom of the same defect: the per-create lens quota at `create_workload.rs:292` counts raw entries, so `Lenses` naming one lens eleven times is refused 402 `ServiceQuotaExceededException` although it attaches one lens — where `associate_lenses.rs` states that what the ceiling counts is "the DISTINCT lenses the workload would end up reviewed against".

## Evidence

Side 1 — the create stores the list raw. `src/ops/create_workload.rs:461` `let mut lenses = input.lenses;`; the alias set is built only for the template merge, `src/ops/create_workload.rs:472-487` — "if !review_template_arns.is_empty() { let mut attached: std::collections::HashSet<String> = lenses.iter().map(|lens| crate::state::lens_alias(lens).to_string()).collect(); ... if attached.insert(crate::state::lens_alias(lens).to_string()) { lenses.push(lens.clone()); }" — and the raw list is what the record keeps, `src/ops/create_workload.rs:560` `lenses,`.

Side 1b — that raw list IS what a read reports: `src/state.rs:2601` `lenses: Some(self.lenses.clone()),` in `to_workload`, and `src/state.rs:2638` `lenses: Some(self.lenses.clone()),` in `to_summary`.

Side 1c — yet the reviews are de-duplicated by lens identity: `src/state.rs:3937-3940` — "The lens reviews a resource applying `lenses` has: every applied lens's [`lens_alias`] — the identity the lens is ONE lens under, so a lens applied under two spellings has exactly one review" / `src/state.rs:3948-3953` `pub fn lens_reviews_of(lenses: &[String]) -> BTreeMap<&str, &str> { ... reviews.entry(lens_alias(lens)).or_insert(lens.as_str()); ... }`. The risk roll-up goes through the same rule: `src/state.rs:2267-2270` — "A lens is attached once however many ways the request named it, so count the questions of each lens the workload reviews once" / `for lens in self.lens_reviews().keys()`.

Side 2 — the sibling attach path applies the de-duplication, with the rule stated: `src/ops/associate_lenses.rs:184-198` — "The lenses the request named, each kept once and in the order it first named them, AS SENT: a lens is one lens however it is spelled (`crate::state::lens_alias` resolves an alias and an ARN to the same lens), so a list naming one twice asks for ONE review, and what the per-workload ceiling counts is the distinct lenses the workload would end up reviewed against." followed by `for lens in &input.lens_aliases { if seen.insert(crate::state::lens_alias(lens)) { named.push(lens.clone()); } }`.

Side 2b — and the crate PINS that stored result for the sibling: `tests/all/workload_lens_association.rs:210-220` — row `case: "the same lens named twice is one review"`, `associated: vec!["serverless".to_string(), official_lens_arn("us-east-1", "serverless")]`, `stored: vec!["wellarchitected".to_string(), "serverless".to_string()]`, asserted against `Workload$Lenses` at `tests/all/workload_lens_association.rs:259-264` through `stored_lenses` (`tests/all/workload_lens_association.rs:71-73`: "The `Workload$Lenses` a stored workload reports through `GetWorkload`").

No test pins the divergent create behaviour: `tests/all/workload_computed_members.rs:92-99` has the row `"framework-lens-named-twice"` with `Lenses: ["wellarchitected", "arn:aws:wellarchitected:us-east-1::lens/wellarchitected"]`, but it asserts only `RiskCounts` (`tests/all/workload_computed_members.rs:117-148`), never `Lenses`; `tests/all/lens_review_listing.rs:345` ("two-spellings-of-one-lens") asserts the review count, not the stored lens list.

## Steps to Reproduce

1. POST /workloads with Lenses: ["wellarchitected", "arn:aws:wellarchitected:us-east-1::lens/wellarchitected"] (both spellings of the one framework lens), plus the required WorkloadName/Description/Environment/ReviewOwner/AwsRegions and a ClientRequestToken
2. GET /workloads/{WorkloadId} — Workload$Lenses carries BOTH entries (two lenses) because src/ops/create_workload.rs:461/560 stores input.lenses verbatim
3. GET /workloads/{WorkloadId}/lensReviews — exactly ONE LensReviewSummary, because src/state.rs:3948 keys the reviews by crate::state::lens_alias; the two answers disagree about how many lenses the workload has
4. Create a second workload with Lenses: ["wellarchitected"] and then PATCH /workloads/{id}/associateLenses with LensAliases: ["serverless", "arn:aws:wellarchitected:us-east-1::lens/serverless"]: GET reports Lenses: ["wellarchitected", "serverless"] — one entry for the twice-named lens, as tests/all/workload_lens_association.rs:218 pins
5. Also: POST /workloads with Lenses naming "wellarchitected" eleven times is refused 402 ServiceQuotaExceededException at src/ops/create_workload.rs:292, although it attaches one lens
