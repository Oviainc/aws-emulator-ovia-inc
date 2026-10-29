Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: cd58d759e0fd70bbd2dd16df1fc60a3010ead2dc

# A profile answer naming a question the published profile template never asks — and choices the question never offers — is stored verbatim, although the crate ships the `offers_choice` judge for exactly that selection and never calls it

_Filed: 2026-10-29T16:50:45Z_
_Operation: CreateProfile | Class: validation_

> _Note: this issue was identified against the code base at commit `cd58d759e0fd70bbd2dd16df1fc60a3010ead2dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateProfile` (and `UpdateProfile`, which shares the same normaliser) records `ProfileQuestions` through `crate::state::recorded_profile_questions`. That helper refuses exactly two shapes of entry — one that names no `QuestionId`, and two entries naming the same `QuestionId` — and then stores whatever `QuestionId` and `SelectedChoiceIds` the request carried, with no resolution against the ONE profile template this emulator publishes (`GetProfileTemplate` / `crate::state::PROFILE_TEMPLATE_QUESTIONS`).

The consequence is a stored profile that answers a question nothing asks: `ProfileRecord::profile_questions` then reports that entry with `QuestionTitle`, `QuestionDescription`, `QuestionChoices`, `MinSelectedChoices` and `MaxSelectedChoices` all ABSENT, because there is no published row to read them from — a `ProfileQuestion` carrying a `SelectedChoiceIds` list and nothing that says what was selected or what the question was. The same holds one level down: a `SelectedChoiceIds` entry that names no choice of a question the template DOES publish is stored and reported back, so `ProfileQuestion$SelectedChoiceIds` can name a choice absent from the `ProfileQuestion$QuestionChoices` catalog served beside it in the very same response.

This is the one place in the crate where an identifier taken from a published catalog is stored without being resolved against it. Every sibling that takes a pillar, a question or a choice of a lens review refuses an undefined one rather than storing it (`undefined_pillar_refusal`, `undefined_pillar_question_refusal`, `undefined_choice_refusal`), and the crate even ships the judge for THIS catalog — `ProfileTemplateQuestionEntry::offers_choice`, documented as "what a selection is judged against" — which no handler, and nothing in `crate::state`, ever calls. `crate::state`'s own comment admits the gap in so many words.

## Evidence

Side 1 — the crate ADMITS the gap, in the doc of the ceiling it put there instead: `src/state.rs:4834-4845` — "/// The most profile questions one profile records here. /// /// In the real service the questions come from the ONE profile template the service /// publishes (`GetProfileTemplate`), so their number is fixed by that template rather /// than by the request. A request here may answer a question [`PROFILE_TEMPLATE_QUESTIONS`] does not publish — nothing refuses one, and such an /// answer is recorded with no published text to report /// ([`ProfileRecord::profile_questions`]) — so this is the ceiling that keeps a stored /// profile's question map bounded …"

Side 2 — the code under it: `src/state.rs:4869-4900` refuses only an absent and a duplicated `QuestionId` and then stores both identifiers verbatim — `questions.insert(question_id.to_string(), update.selected_choice_ids.clone().unwrap_or_default().into_iter().collect());` — with no lookup of `profile_template_question(question_id)` and no check of the choices. Both writers go through it: `src/ops/create_profile.rs:174-175` — "let questions = crate::state::recorded_profile_questions(&input.profile_questions).map_err(CreateProfileErrorModeled::ValidationException)?;" — and `src/ops/update_profile.rs:165-169`.

Side 3 — the judge the crate already has, and never calls: `src/state/profile_template.rs:136-139` — "/// Whether `choice_id` names one of this question's choices — a choice belongs to the /// question that offers it, so this is what a selection is judged against. /// pub fn offers_choice(&self, choice_id: &str) -> bool {". A repo-wide grep for `offers_choice` matches only that definition; the only caller of `profile_template_question(` at all is the REPORTING projection, `src/state.rs:5033`.

Side 4 — what the read then reports: `src/state.rs:5027-5044` — "/// A question the template does not publish has no published text, so for it those /// members are reported ABSENT rather than invented." with `question_title: published.map(|question| question.title.to_string())`, `question_choices: published.map(ProfileTemplateQuestionEntry::choice_catalog)`, … so the stored answer is served as a question with no text and no choice catalog.

Side 5 — the sibling rule this path skips. `src/ops/update_answer.rs:253-262` — "// A `ChoiceId` is a choice of THIS question (`Answer$Choices` is the only place a // client discovers them), so one that names no choice of it belongs to no answer // of this question and is refused rather than stored." with `crate::state::undefined_choice_refusal(&question, input.selected_choices.as_deref(), input.choice_updates.as_ref())`. `src/ops/update_lens_review.rs:218-231` — "// A `PillarNotes` key is a `PillarId`, and the pillars of a lens are what its lens // content names: a key naming a pillar the lens does NOT define belongs to no // pillar of this lens review and is refused rather than stored." The profile template is the same kind of published catalog: `src/state/profile_template.rs:82-84` — "/// `ProfileTemplateQuestion$QuestionId` — the key of the table, and what a profile's /// answer is recorded under." and `:68-70` — "/// `ProfileTemplateChoice$ChoiceId` — what a request's `SelectedChoiceIds` names the /// choice by, unique within the question that offers it."

Test coverage today: `tests/all/profile_template.rs` walks the published template, `tests/all/profile_lifecycle.rs` and `tests/all/profile_revisions.rs` exercise create/update with template question ids — none sends an id the template does not publish, so no test pins today's acceptance.

## Steps to Reproduce

1. GET /profileTemplate (GetProfileTemplate) and note the `TemplateQuestions[].QuestionId` values and each question's `QuestionChoices[].ChoiceId` — the only place a client discovers either (src/state/profile_template.rs:151, PROFILE_TEMPLATE_QUESTIONS)
2. POST /profiles with ProfileQuestions = [{"QuestionId": "not-a-template-question", "SelectedChoiceIds": ["not-a-choice"]}] (plus ProfileName/ProfileDescription/ClientRequestToken)
3. Observe 200 and a minted ProfileArn: nothing refused the entry (src/state.rs:4869-4900)
4. GET /profiles/{ProfileArn} (GetProfile): the profile reports a ProfileQuestion whose QuestionId is `not-a-template-question`, whose SelectedChoiceIds is ["not-a-choice"], and whose QuestionTitle, QuestionDescription, QuestionChoices, MinSelectedChoices and MaxSelectedChoices are all absent (src/state.rs:5029-5047)
5. Repeat with a question id the template DOES publish but a SelectedChoiceIds entry it does not offer: the selection is stored and served beside a QuestionChoices catalog that does not contain it, although ProfileTemplateQuestionEntry::offers_choice (src/state/profile_template.rs:138) exists to refuse exactly that and is never called
6. Contrast with the sibling catalog write: PATCH /workloads/{WorkloadId}/lensReviews/wellarchitected/answers/ops-01 with {"SelectedChoices": ["not-a-choice"]} is refused 400 ValidationException by crate::state::undefined_choice_refusal (src/ops/update_answer.rs:256-262)
7. PATCH /profiles/{ProfileArn} (UpdateProfile) with the same unpublished entry: accepted the same way, through the same helper (src/ops/update_profile.rs:165-169)
