Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6f4880dda1aa7f89588445b4a5ba9a411741294f

# ListAnswers reports an empty answer list for a workload lens review of an imported custom lens

_Filed: 2026-10-24T17:07:07Z_
_Operation: ListAnswers | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6f4880dda1aa7f89588445b4a5ba9a411741294f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A published custom lens can be attached to a workload at CreateWorkload (the handler resolves it through `unapplicable_lenses_refusal`, and the lens-resolution test asserts that a published custom lens is applicable to a workload over HTTP). Once attached, `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers` must report one `AnswerSummary` per question the custom lens defines — each with its `QuestionId`, `PillarId`, `QuestionTitle`, the lens's `Choices` catalog, `Risk=UNANSWERED` and `IsApplicable=true`.

The emulator instead answers 200 with `AnswerSummaries: []` and a `NextToken` of none, because the question set is read from the AWS-official static catalog only; the custom lens's pillars/questions/choices that ImportLens already parsed and stored are never consulted. A client that imports a lens, publishes it, applies it to a workload and then lists the review's answers sees a review with no questions at all, and `PillarId`/`QuestionPriority` filtering over it can never match anything.

## Evidence

Spec: `ListAnswers` returns `AnswerSummaries` for the lens review named by `LensAlias`, where "LensAlias" is "either the lens alias ... or the lens ARN" of a lens applied to the workload (`smithy_query(query="operation", operation="ListAnswers")`, `LensAlias` doc). ImportLens is documented as defining the lens content (pillars, questions, choices) that reviews of that lens ask.

Handler: `src/ops/list_answers.rs:239-251` pages over `crate::state::lens_questions(&input.lens_alias)`; `src/state/official_lenses.rs:267-269` defines `lens_questions` as `official_lens(lens).map_or(&[][..], ...)` and `official_lens` returns `None` for anything that is not an AWS-official alias/ARN — so a custom lens yields an empty slice. The content IS in the store: `ImportLens` parses it into `LensDefinition` (`src/ops/import_lens.rs:186,201-205`) and `GetLensVersionDifference` walks those same pillars/questions (`definition_pillars`, `src/ops/get_lens_version_difference.rs:354-388`).

Attachment is reachable over HTTP: `tests/all/lens_resolution.rs:365-384` asserts a published custom lens is accepted by CreateWorkload and reported on the stored workload. No test asserts the empty answer page for a custom lens. The sibling defect on the single-review read is already filed (GetLensReview wrong-behavior, 20261024T063221Z) — this is the distinct answer-listing response.

## Steps to Reproduce

1. ImportLens with a valid lens document; wait for the import to land
2. CreateLensVersion to publish version 1.0 of that lens
3. CreateWorkload with Lenses=[the custom lens ARN] (plus the required ReviewOwner/regions) — accepted
4. GET /workloads/{WorkloadId}/lensReviews/{url-encoded lens ARN}/answers — 200 with AnswerSummaries: [] instead of one summary per question of the imported lens (each UNANSWERED, with the lens's Choices)
