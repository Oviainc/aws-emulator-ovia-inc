Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# A review template's lens review of an imported CUSTOM lens reports no pillars, no lens name/version/status and all-zero QuestionCounts

_Filed: 2026-10-24T06:32:21Z_
_Operation: GetReviewTemplateLensReview | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateReviewTemplate` accepts a published custom lens in `Lenses` (it resolves the list through `crate::state::unapplicable_lenses_refusal`, which returns `LensResolution::PublishedCustom`), so a template can legitimately apply a lens the caller imported with `ImportLens` and published with `CreateLensVersion`. The imported document's pillars and questions are parsed and held on the `LensRecord` (`LensContent::definition`).

But `ReviewTemplateRecord::to_lens_review` derives everything about the lens from the framework-only static helpers: `lens_metadata(lens)` (so `LensName`/`LensVersion`/`LensStatus` are omitted), `Self::lens_review_pillars(lens)` -> `lens_pillars(lens)` (so `PillarReviewSummaries` is empty — only pillars the caller wrote notes for appear, and then with no `PillarName`), and `Self::lens_question_counts` / `pillar_question_counts` -> `lens_questions(lens)` (so every `QuestionCounts` bucket, `UNANSWERED` included, is 0).

Expected: `GetReviewTemplateLensReview` (and `UpdateReviewTemplateLensReview`, which returns the same shape) reports the custom lens's name/version/status, one `ReviewTemplatePillarReviewSummary` per pillar the imported document defines, and `QuestionCounts.UNANSWERED` equal to the number of questions that lens asks on a template nothing has answered yet.

Actual: 200 with `LensName`/`LensVersion`/`LensStatus` absent, `PillarReviewSummaries` = `[]` and all `QuestionCounts` buckets 0 — the template claims the applied lens asks no questions.

Same root cause, same resource family: `ListReviewTemplateAnswers` for that template and custom lens answers 200 with `AnswerSummaries: []` instead of one `ReviewTemplateAnswerSummary` per question the document defines, and `GetReviewTemplate`'s `ReviewTemplate$QuestionCounts` roll-up is 0 for the same reason. Making `lens_questions`/`lens_pillars`/`lens_metadata` resolve a custom lens through the store fixes them together.

## Evidence

Smithy model: `ReviewTemplateLensReview` declares `LensName`, `LensVersion`, `LensStatus`, `PillarReviewSummaries` (list of `ReviewTemplatePillarReviewSummary{PillarId, PillarName, Notes, QuestionCounts}`) and `QuestionCounts`; `ReviewTemplateAnswerSummary$AnswerStatus` is "the status of whether or not this question has been answered", i.e. the questions of the applied lens are the template's question set. `CreateReviewTemplateInput$Lenses` doc: "Lenses applied to the review template".

Emulator: `src/ops/get_review_template_lens_review.rs` lines 197-201 and `src/ops/update_review_template_lens_review.rs` lines 268-272 both call `ReviewTemplateRecord::to_lens_review`; `src/state.rs` `to_lens_review` takes its lens facts from `lens_metadata(lens)` and `Self::lens_review_pillars(lens)`, whose own doc concedes "for a lens it holds none for the first group is [empty], because such a lens is organized into no pillars HERE while the real lens is"; `lens_question_counts`/`pillar_question_counts` count over `lens_questions(lens)`, which is `&[]` for any alias but `wellarchitected`. `src/ops/list_review_template_answers.rs` lines 219-241 page over the same `crate::state::lens_questions`.

Reachability: `tests/all/lens_resolution.rs` lines 296-365 assert that "a published custom lens must be applicable to a review template" and that "the stored template must report the custom lens as sent".

Test coverage: `tests/all/review_template_lens_review.rs` and `tests/all/review_template_answers.rs` exercise only `wellarchitected` (shipped) and `serverless` (unshipped AWS-official, `const UNSHIPPED_LENS`); no test asserts the content of a custom lens's template review.

## Steps to Reproduce

1. Boot the wellarchitected emulator
2. PUT /importLens with a valid lens JSON document defining two pillars with three questions each; wait for the import to land
3. POST /lenses/{LensArn}/versions with LensVersion="1.0" to publish it
4. POST /reviewTemplates (CreateReviewTemplate) with Lenses=["<LensArn>"] and note the TemplateArn
5. GET /reviewTemplates/{TemplateArn}/lensReviews/{LensArn} (GetReviewTemplateLensReview): observe 200 with LensName/LensVersion/LensStatus absent, PillarReviewSummaries = [] and all QuestionCounts buckets 0, instead of the two pillars and UNANSWERED = 6
6. GET /reviewTemplates/{TemplateArn}/lensReviews/{LensArn}/answers (ListReviewTemplateAnswers): observe 200 with AnswerSummaries = [] instead of the six questions the imported document defines
7. GET /reviewTemplates/{TemplateArn} (GetReviewTemplate): observe ReviewTemplate.QuestionCounts all 0 for the same reason
