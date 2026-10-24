Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# Lens catalog holds only the `wellarchitected` lens, so ListLenses omits every other AWS-official lens

_Filed: 2026-10-24T01:59:23Z_
_Operation: ListLenses | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The emulator's lens catalog contains exactly ONE AWS-official lens, the Well-Architected Framework lens (`wellarchitected`), plus whatever custom lenses `ImportLens` created. Real AWS publishes a whole catalog of official lenses — `serverless`, `softwareasaservice`, `iot`, `machinelearning`, `analytics`, `financialservices`, `sap`, `migration`, `genai`, … — each with its own `LensName`, `LensVersion`, pillars and question set.

Expected: `ListLenses` (and `ListLenses?LensType=AWS_OFFICIAL`) returns a `LensSummary` for each AWS-official lens the service publishes, filterable by `LensName`, and the same lenses are reviewable on a workload. Actual: `ListLenses` returns a single AWS-official summary; `LensName="Serverless Applications Lens"` returns an empty page.

Knock-on effects of the same missing catalog, visible on other operations: a workload created with `Lenses=["serverless"]` reports a lens review with NO `LensName`, `LensVersion` or `LensStatus`, an empty `PillarReviewSummaries` and an all-zero risk roll-up (`get_lens_review.rs` lines 197-248 derive these from `crate::state::lens_pillars`/`lens_metadata`, which return `&[]`/`None` for any alias but `wellarchitected`), and `ListAnswers` for that lens returns an empty `AnswerSummaries` with a 200 instead of that lens's questions.

## Evidence

Spec: `LensAlias` doc names `serverless` (alias and `arn:aws:wellarchitected:us-east-1::lens/serverless`) as an Amazon Web Services official lens; `LensType` publishes `AWS_OFFICIAL`; `ListLenses` is `@paginated` over `LensSummaries` and filters by `LensType`/`LensName`/`LensStatus`. Emulator: `src/ops/list_lenses.rs` lines 220-227 push exactly one AWS-official catalog entry (`crate::state::framework_lens_summary`); `src/state.rs` `lens_questions`, `lens_pillars` and `lens_metadata` all return empty/`None` unless `lens_alias(lens) == FRAMEWORK_LENS_ALIAS` ("Only the AWS-official Well-Architected Framework lens has a definition here"). Test coverage: the current (incomplete) behaviour is pinned by `tests/all/workload_computed_members.rs` line 87 (`("unshipped-lens-only", json!(["serverless"]), 0)`) and `tests/all/lens_review_answers.rs` line 48 (`const UNSHIPPED_LENS: &str = "serverless"`), i.e. no test asserts the real catalog — they assert the gap.

## Steps to Reproduce

1. Boot the wellarchitected emulator
2. Call ListLenses with LensType=AWS_OFFICIAL and observe exactly one LensSummary (`wellarchitected`); real AWS returns the whole official catalog
3. Call ListLenses with LensName="Serverless Applications Lens" and observe an empty page
4. Call CreateWorkload with Lenses=["serverless"], then GetLensReview for LensAlias=serverless: observe 200 with no LensName/LensVersion/LensStatus, empty PillarReviewSummaries and an all-zero risk roll-up
5. Call ListAnswers for the same workload and lens and observe an empty AnswerSummaries list
