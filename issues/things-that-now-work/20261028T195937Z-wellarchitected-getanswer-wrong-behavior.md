Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 099b155a5bca3b1c2e4dd48b7f9cfde84b7dc96c

# An answer's Risk and ChoiceAnswers are computed over the stored choice map instead of the question's own choice set, so a lens-version upgrade leaves the question permanently mis-rated

_Filed: 2026-10-28T19:59:37Z_
_Operation: GetAnswer | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `099b155a5bca3b1c2e4dd48b7f9cfde84b7dc96c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AnswerRecord::risk()` is the one place a question's `Risk` is derived (it feeds `GetAnswer`, `ListAnswers`, `GetLensReview`'s risk roll-ups and the review-template equivalents). It takes NO question: it decides MEDIUM vs NONE by scanning every entry that happens to be stored in `AnswerRecord::choice_answers`. Every other operation on the same record is scoped by the question's own choice catalog — `AnswerRecord::for_question`, `select_only` and `resolve_selected_choices` all walk `question.each_choice_id()`, which the crate documents as "the key space an answer's per-choice statuses live in".

The stored map is populated from the question's choice set as it stood when the question was FIRST answered (`for_question`) and is never re-scoped afterwards. `UpgradeLensReview` / `UpgradeReviewTemplateLensReview` move the review to a newer lens version — a custom lens version whose document may offer a different choice set for the same `QuestionId` — and touch nothing else, so the stored map and the question's catalog drift apart. Two wrong answers follow, in opposite directions:

* a version that ADDS a choice: the new `ChoiceId` has no stored entry, so `risk()` finds nothing UNSELECTED and reports `NONE` (no risk) for a question whose newly asked best practice has never been selected;
* a version that DROPS a choice: the dropped `ChoiceId`'s stored `UNSELECTED` entry still counts, so the question is stuck at `MEDIUM` even though every best practice the review's lens version asks about is selected — and no request can ever clear it, because `UpdateAnswer` refuses a `ChoiceId` the question does not define (`undefined_choice_refusal`).

The same drift makes one response self-contradictory: `Answer$Choices` / `AnswerSummary$Choices` are projected from `question.choices()` while `ChoiceAnswers` / `ChoiceAnswerSummaries` are projected from the stored map, so the reply lists a `ChoiceAnswer` for a `ChoiceId` that does not appear in the `Choices` catalog of the same answer.

## Evidence

The rule the crate states for the key space, src/state.rs:448-451:
```
    /// The `ChoiceId` of every best practice this question asks about, in the lens's own
    /// order — the key space an answer's per-choice statuses live in, and what a
    /// request's `SelectedChoices` values and `ChoiceUpdates` keys are judged against.
```
Two siblings honour it — src/state.rs:1380 (`select_only`) `for choice_id in question.each_choice_id() {` and src/state.rs:1403-1409 (`resolve_selected_choices`):
```
    pub fn resolve_selected_choices(&mut self, question: &ReviewQuestion) {
        self.selected_choices = question
            .each_choice_id()
            .filter(|choice_id| {
```
The risk derivation does not — src/state.rs:1423-1437, which also states the rule it breaks ("one where some choice is still unselected is `MEDIUM`, and one with nothing left unselected is `NONE`", of the question):
```
    pub fn risk(&self) -> &'static str {
        if !self.is_applicable {
            return RISK_NOT_APPLICABLE;
        }
        if self.selected_choices.is_empty() {
            return RISK_HIGH;
        }
        if self
            .choice_answers
            .values()
            .any(|choice| choice.status == CHOICE_STATUS_UNSELECTED)
        {
            return RISK_MEDIUM;
        }
        RISK_NONE
```
The map is frozen at first-answer time — src/state.rs:1349-1353:
```
    pub fn for_question(question: &ReviewQuestion) -> Self {
        Self {
            selected_choices: Vec::new(),
            choice_answers: question
                .each_choice_id()
```
and the upgrade that re-points the review at a different choice set writes only the version — src/ops/upgrade_lens_review.rs:318-320:
```
            workload
                .lens_review_record_mut(&input.lens_alias)
                .lens_version = latest;
```
(the same for templates, src/ops/upgrade_review_template_lens_review.rs:289-290: `let review = template.lens_review_record_mut(&input.lens_alias, now); review.lens_version = latest;`). Nothing re-scopes the map: `grep choice_answers` finds only the constructor, the two question-scoped writers, the two `ChoiceUpdates` inserts and the four projections — no pruning anywhere.

The contradictory projection, src/state.rs:1471-1492 (`answer_projection`): `choices: Some(question.choices()),` beside `choice_answers: Some(answer.map(|answer| answer.choice_answers.iter().map(...)))`; identical in src/ops/list_answers.rs:313-331 and in the review-template projection src/state.rs:3262-3283.

Test coverage: `tests/custom_lens_versions`, `tests/workload_lens_review_upgrade`, `tests/lens_review_pinned_version_content`, `tests/lens_review_rollup`, `tests/get_answer_reads` and `tests/custom_lens_answer_writes` exercise custom-lens versions, the upgrade and the answer roll-up, but none pins a question whose CHOICE SET changed between two published versions, so nothing covers this case.

## Steps to Reproduce

1. ImportLens (no LensAlias) with a lens document defining one pillar and one question Q offering choices c1 and c2; CreateLensVersion LensVersion="1" on the returned lens ARN.
2. CreateWorkload with Lenses=[that lens ARN].
3. UpdateAnswer for Q with SelectedChoices=[c1, c2]; GetAnswer reports Answer$Risk = NONE.
4. ImportLens again with LensAlias=<the same lens ARN> and a document where Q offers c1, c2 and c3; CreateLensVersion LensVersion="2".
5. UpgradeLensReview on the workload/lens with a fresh MilestoneName.
6. GetAnswer for Q: Answer$Choices lists c1, c2, c3 but Answer$Risk is still NONE, although c3 — a best practice the review's lens version now asks about — has never been selected (and GetLensReview's RiskCounts counts the question as NONE).
7. Mirror case: repeat steps 1-5 publishing version "2" with Q offering c1 only. GetAnswer then reports Risk = MEDIUM forever even though the only choice the question asks about (c1) is SELECTED, and ChoiceAnswers carries an entry for c2 — a ChoiceId absent from the same answer's Choices catalog, which UpdateAnswer refuses as undefined, so the rating cannot be corrected.
