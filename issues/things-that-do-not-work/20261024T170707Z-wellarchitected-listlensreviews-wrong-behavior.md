Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6f4880dda1aa7f89588445b4a5ba9a411741294f

# ListLensReviews reports a custom lens review with no LensName/LensVersion/LensStatus and an all-zero risk roll-up

_Filed: 2026-10-24T17:07:07Z_
_Operation: ListLensReviews | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6f4880dda1aa7f89588445b4a5ba9a411741294f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For a workload that has a published custom lens attached, `GET /workloads/{WorkloadId}/lensReviews` must report a `LensReviewSummary` carrying the lens's name and published version (`LensName`, `LensVersion`), its `LensStatus`, and `RiskCounts` whose `UNANSWERED` bucket equals the number of questions that lens asks.

The emulator reports the summary with `LensName`, `LensVersion` and `LensStatus` absent and every `RiskCounts` bucket at 0, because both are derived from the AWS-official static catalog rather than from the stored custom-lens content. The listing therefore claims a lens review that asks no questions and identifies no lens, while ImportLens/CreateLensVersion have both the name and the version on the record.

## Evidence

Spec: `LensReviewSummary` models `LensAlias`, `LensArn`, `LensVersion`, `LensName`, `LensStatus`, `UpdatedAt`, `RiskCounts` (`smithy_query(query="shape", shape="LensReviewSummary")`); a custom lens's name/version are the content ImportLens imported and CreateLensVersion published.

Handler: `src/ops/list_lens_reviews.rs:254-292` — `let metadata = crate::state::lens_metadata(lens);` then `lens_version/lens_name/lens_status: metadata.map(...)`, and `risk_counts: Some(risk_counts_to_map(&workload.lens_risk_counts(lens)))`. `src/state/official_lenses.rs:292-294` returns `None` from `lens_metadata` for any non-official lens, and `WorkloadRecord::lens_risk_counts` (`src/state.rs:~1515`) rolls up over `lens_questions(lens)`, which is empty for a custom lens.

The stored custom lens does carry a name and versions (`LensRecord.versions`, `summary_content().definition.name` is already used for lens share invitations in `src/state.rs:~4406`). Attachment over HTTP is asserted by `tests/all/lens_resolution.rs:365-384`; `tests/all/lens_review_listing.rs` covers the official lens only, so nothing pins the hollow custom-lens summary.

## Steps to Reproduce

1. ImportLens a lens document and wait for the import to land, then CreateLensVersion to publish version 1.0
2. CreateWorkload with Lenses=[the custom lens ARN]
3. GET /workloads/{WorkloadId}/lensReviews — the summary for the custom lens omits LensName, LensVersion and LensStatus and reports RiskCounts with every bucket 0, instead of the imported lens's name/version and UNANSWERED equal to its question count
