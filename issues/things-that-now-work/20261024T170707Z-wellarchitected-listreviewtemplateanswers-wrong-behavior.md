Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6f4880dda1aa7f89588445b4a5ba9a411741294f

# ListReviewTemplateAnswers returns an empty answer list for a review template's custom-lens review

_Filed: 2026-10-24T17:07:07Z_
_Operation: ListReviewTemplateAnswers | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6f4880dda1aa7f89588445b4a5ba9a411741294f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateReviewTemplate accepts a published custom lens in `Lenses` (resolved through the same `unapplicable_lenses_refusal` path, asserted over HTTP by the lens-resolution suite). `GET /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}/answers` must then report one `ReviewTemplateAnswerSummary` per question that custom lens defines, each `UNANSWERED` with the lens's `Choices` catalog.

The emulator answers 200 with `AnswerSummaries: []`, because the question set comes from the AWS-official static catalog only and a custom lens resolves to an empty question slice there. A template built on a custom lens therefore reports a lens review with no questions, and the `PillarId` filter can never select anything.

## Evidence

Spec: `ListReviewTemplateAnswers` output models `AnswerSummaries` (list<ReviewTemplateAnswerSummary>) for the lens applied to the template (OPERATIONS.md, ListReviewTemplateAnswers).

Handler: `src/ops/list_review_template_answers.rs:219-230` pages over `crate::state::lens_questions(&input.lens_alias)`; `src/state/official_lenses.rs:267-269` returns an empty slice for every non-official lens, although ImportLens stored that lens's pillars/questions/choices (`src/ops/import_lens.rs:186,201-205`, walked by `definition_pillars` in `src/ops/get_lens_version_difference.rs:354-388`).

Reachability: `tests/all/lens_resolution.rs:358-365` asserts a published custom lens is accepted by CreateReviewTemplate and reported on the stored template. `tests/all/review_template_answers.rs` exercises the official lens only. The sibling single-read defect is filed separately (GetReviewTemplateLensReview wrong-behavior, 20261024T063221Z); this is the distinct answer-listing response.

## Steps to Reproduce

1. ImportLens a lens document, wait for the import to land, then CreateLensVersion to publish version 1.0
2. CreateReviewTemplate with Lenses=[the custom lens ARN] — accepted
3. GET /reviewTemplates/{url-encoded TemplateArn}/lensReviews/{url-encoded lens ARN}/answers — 200 with AnswerSummaries: [] instead of one UNANSWERED summary per question of the imported lens
