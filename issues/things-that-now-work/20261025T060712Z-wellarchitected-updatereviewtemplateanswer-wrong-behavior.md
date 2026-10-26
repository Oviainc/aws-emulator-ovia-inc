Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# UpdateReviewTemplateAnswer resolves the question against the AWS-official catalog only, so a review template's custom-lens review cannot be answered

_Filed: 2026-10-25T06:07:12Z_
_Operation: UpdateReviewTemplateAnswer | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateReviewTemplateAnswer resolves the `QuestionId` through the official-lens-only free function `crate::state::lens_question`, while the sibling read GetReviewTemplateAnswer (and ListReviewTemplateAnswers) resolve the same question set through the custom-lens-aware `store.lens_review_question(s)`. A published custom lens can be applied to a review template (CreateReviewTemplate resolves the lenses through `unapplicable_lenses_refusal`), and the template's lens review then reports the questions of the imported document — but every attempt to answer one of them is refused with a 404, so a review template's custom-lens review can be read and never written. The handler's own module doc states the rule the code breaks.

## Evidence

Handler doc states the rule: `services/.../src/ops/update_review_template_answer.rs:20-23` — "A `TemplateArn` nothing is stored under, a lens the template does not apply and a `QuestionId` that lens does not ask are each a modelled 404". Handler code implements "a QuestionId the OFFICIAL CATALOG does not ask": `update_review_template_answer.rs:233-246` — "// A question belongs to the LENS under review: the answer this operation writes is the answer to one of the questions that lens asks" then `let question = crate::state::lens_question(&input.lens_alias, &input.question_id)` … `.ok_or_else(|| ...ResourceNotFoundException(crate::state::review_template_answer_not_found(...)))`.

`crate::state::lens_question` is official-only by construction: `src/state/official_lenses.rs:247-250` (`if !is_aws_official_lens(lens) { return None; }`), `:262-266` ("A CUSTOM lens's questions are not here … an operation that reports the questions of whatever lens a request named resolves them through [`lens_review_questions`]"), `:284-288`.

Sibling read of the same answer uses the custom-lens-aware lookup: `src/ops/get_review_template_answer.rs:201-214` — "// A question belongs to the LENS under review — the questions that lens's own content defines (`crate::state::...::lens_review_question`)" then `store.lens_review_question(&input.lens_alias, &input.question_id)`; and `src/ops/list_review_template_answers.rs:215-222` pages over `store.lens_review_questions(&input.lens_alias)` ("an AWS-official lens's catalog entry or a custom lens's imported document").

Unpinned: `grep -n 'custom|import' tests/all/review_template_answer_updates.rs` returns no matches, and `tests/all/custom_lens_review_answers.rs:17-19` covers only the three READ paths.

## Steps to Reproduce

1. ImportLens with a valid lens document and wait for the import to land
2. CreateLensVersion to publish version 1.0 of that custom lens
3. CreateReviewTemplate with Lenses=[the custom lens ARN] — accepted
4. GET /reviewTemplates/{TemplateArn}/lensReviews/{url-encoded lens ARN}/answers — 200 with one summary per question of the imported document
5. PATCH .../answers/{QuestionId} (UpdateReviewTemplateAnswer) with SelectedChoices from that summary — 404 ResourceNotFoundException instead of recording the answer
