Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# UpdateAnswer resolves the question against the AWS-official catalog only, so no question of a custom lens review can ever be answered

_Filed: 2026-10-25T06:07:12Z_
_Operation: UpdateAnswer | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateAnswer resolves the `QuestionId` it is asked to answer through the official-lens-only free function `crate::state::lens_question`, while every READ of the same lens review (GetAnswer, ListAnswers, ListLensReviewImprovements) resolves it through the custom-lens-aware `store.lens_review_question(s)`. A published custom lens can be attached to a workload (CreateWorkload / AssociateLenses resolve it through `unapplicable_lenses_refusal`), and ListAnswers/GetAnswer then report one answer per question the imported lens document defines. UpdateAnswer refuses every one of those `QuestionId`s with a 404 ResourceNotFoundException, because `lens_question` returns `None` for anything that is not an AWS-official alias/ARN. The consequence: a custom lens review is readable but unanswerable — the answers a client just listed can never be written, so the risk roll-up of a custom lens review stays UNANSWERED forever. The handler's own comment states the rule it breaks: the question has to be one "that lens asks", and for a custom lens that set is the imported document, not the static catalog.

## Evidence

Handler (uses the official-catalog lookup): `services/.../src/ops/update_answer.rs:216-229` — "// A question belongs to the LENS under review: the answer this operation writes is the answer to one of the questions that lens asks, so a `QuestionId` it does not\n// ask names no answer of this lens review." then `let question = crate::state::lens_question(&input.lens_alias, &input.question_id)` … `.ok_or_else(|| UpdateAnswerErrorModeled::ResourceNotFoundException(crate::state::question_not_found(...)))`.

That helper is documented as official-only: `services/.../src/state/official_lenses.rs:262-266` — "A CUSTOM lens's questions are not here — its question set is lens CONTENT, held on its own [`super::LensRecord`] in the store — so an operation that reports the questions of whatever lens a request named resolves them through [`...::lens_review_questions`], which is this table for an official lens and the imported document for a custom one." and `official_lenses.rs:284-288` `pub fn lens_question(...) { lens_questions(lens).iter().find(...) }` over `official_lens(lens).map_or(&[][..], ...)` (`official_lenses.rs:247-250`: `if !is_aws_official_lens(lens) { return None; }`).

Sibling read on the SAME resource uses the custom-lens-aware lookup: `services/.../src/ops/get_answer.rs:208-219` — "// A question belongs to the LENS under review — the questions that lens's own\n// content defines (`crate::state::...::lens_review_question`)" then `store.lens_review_question(&input.lens_alias, &input.question_id)`; and `src/state.rs:4902-4922` defines `lens_review_questions` as the catalog for an official lens and `LensRecord::reviewed_content → LensDefinition::review_questions()` for a custom one.

No test pins the refusal: `tests/all/custom_lens_review_answers.rs:17-19` states "The reads covered are the answer list of a workload's lens review (`ListAnswers`), the single answer read (`GetAnswer`) and the answer list of a review template's lens review (`ListReviewTemplateAnswers`)" — the write paths are absent, and `grep -n custom tests/all/answer_updates.rs` returns no matches.

## Steps to Reproduce

1. ImportLens with a valid lens document and wait for the import to land
2. CreateLensVersion to publish version 1.0 of that custom lens
3. CreateWorkload with Lenses=[the custom lens ARN] (plus ReviewOwner and a Region) — accepted
4. GET /workloads/{WorkloadId}/lensReviews/{url-encoded lens ARN}/answers — 200 with one AnswerSummary per question of the imported document
5. PATCH /workloads/{WorkloadId}/lensReviews/{url-encoded lens ARN}/answers/{QuestionId} (UpdateAnswer) with SelectedChoices from that summary's Choices — 404 ResourceNotFoundException instead of recording the answer, so the question stays UNANSWERED
