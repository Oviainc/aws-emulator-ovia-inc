Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# A workload lens review of an imported CUSTOM lens reports no pillars, no lens name/version/status and an all-zero risk roll-up, although ImportLens parsed and stored that lens's pillars and questions

_Filed: 2026-10-24T06:32:21Z_
_Operation: GetLensReview | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The emulator fully supports the custom-lens lifecycle: `ImportLens` parses a lens JSON document into a `crate::state::LensDefinition` (schema version, name, description, `pillars` -> `LensPillar` -> `questions` -> `LensQuestion`/`LensChoice`/`LensRiskRule`), `CreateLensVersion` publishes a version from it, and `CreateWorkload` accepts that published custom lens in `Lenses` (`state::unapplicable_lenses_refusal` returns `LensResolution::PublishedCustom`, so the workload is created with a review of that lens).

But every READ of that lens review throws the stored definition away. `GetLensReview` derives its answer from `crate::state::lens_pillars(lens)` and `crate::state::lens_metadata(lens)`, both of which are static functions keyed on `lens_alias(lens) == FRAMEWORK_LENS_ALIAS` and return `&[]` / `None` for anything else, and `WorkloadRecord::lens_risk_counts` / `pillar_risk_counts` count over `crate::state::lens_questions(lens)`, which is `&[]` for a custom lens.

Expected (real service): `GetLensReview` for a published custom lens attached to a workload reports `LensReview$LensName`, `LensVersion` and `LensStatus` from the imported lens document, one `PillarReviewSummary` per pillar the document defines (with `PillarId`/`PillarName`), and a `RiskCounts` roll-up whose `UNANSWERED` bucket equals the number of questions that lens asks (every question is UNANSWERED on a fresh workload).

Actual: HTTP 200 with `LensName`, `LensVersion` and `LensStatus` absent, `PillarReviewSummaries` = `[]`, and every `RiskCounts` bucket (including `UNANSWERED`) = 0 — a review that claims the custom lens asks nothing.

Same root cause, same resource family: `ListAnswers` for that workload and custom lens answers 200 with `AnswerSummaries: []` instead of one `AnswerSummary` per question the imported document defines (it pages over the same `crate::state::lens_questions`). Making the three lens-content helpers resolve a custom lens through the store (`LensRecord::settled_content` / `published_version` -> `LensContent::definition`) fixes both reads at once.

Note this is distinct from the already-open ListLenses issue (`20261024T015924Z-wellarchitected-listlenses-wrong-behavior`), which is about AWS-OFFICIAL lenses the emulator ships no content for (`serverless`, `iot`, …). Here the content IS present in the store, imported by the caller, and is simply never projected into a lens review.

## Evidence

Smithy model: `LensReview` declares `LensName`, `LensVersion`, `LensStatus`, `PillarReviewSummaries` (list of `PillarReviewSummary{PillarId, PillarName, Notes, RiskCounts, PrioritizedRiskCounts}`) and `RiskCounts`; `ListAnswers` is `@paginated` over `AnswerSummaries`. `ImportLens` doc: "Lenses are defined in JSON" and "The lens cannot be applied to workloads … until it's published with CreateLensVersion" — i.e. once published it IS applicable and its questions are the review's questions.

Emulator: `src/ops/get_lens_review.rs` lines 193-248 build `pillar_review_summaries` from `crate::state::lens_pillars(lens)` and `lens_version`/`lens_name`/`lens_status` from `crate::state::lens_metadata(lens)`; `src/state.rs` `lens_questions` ("if lens_alias(lens) == FRAMEWORK_LENS_ALIAS { FRAMEWORK_LENS_QUESTIONS } else { &[] }"), `lens_pillars` and `lens_metadata` are all framework-only and take no store reference. `src/state.rs` `WorkloadRecord::lens_risk_counts` / `pillar_risk_counts` count over `lens_questions(lens)`. The parsed definition exists but is only consumed by `LensRecord::to_summary` for `ListLenses` (`content.definition.name` / `.description`) — `LensDefinition::pillars` is never read by any review path.

Reachability: `tests/all/lens_resolution.rs` lines 296-384 assert that a published custom lens IS applicable to a workload and that "the stored workload must report the custom lens as sent", so a caller reaches this state over plain HTTP.

Test coverage: no test asserts a custom lens's review content. `tests/all/lens_review_answers.rs` and `tests/all/lens_review_rollup.rs` only use `wellarchitected` (shipped) and `serverless` (unshipped official).

## Steps to Reproduce

1. Boot the wellarchitected emulator
2. PUT /importLens with a valid lens JSON document defining, say, two pillars with three questions each; note the returned LensArn and wait for the import to land
3. POST /lenses/{LensArn}/versions (CreateLensVersion) with LensVersion="1.0" to publish it
4. POST /workloads (CreateWorkload) with Lenses=["<LensArn>"] — succeeds, so the workload carries a review of the custom lens
5. GET /workloads/{WorkloadId}/lensReviews/{LensArn} (GetLensReview): observe 200 with LensName/LensVersion/LensStatus absent, PillarReviewSummaries = [] and every RiskCounts bucket 0, instead of the two pillars and an UNANSWERED count of 6
6. GET /workloads/{WorkloadId}/lensReviews/{LensArn}/answers (ListAnswers): observe 200 with AnswerSummaries = [] instead of the six questions the imported document defines
