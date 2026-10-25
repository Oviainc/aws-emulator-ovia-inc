Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dd185e64193b12242704f82dcc412d70cd33b9eb

# UpdateAnswer with no writable member materialises a full answer record, flipping the question from UNANSWERED to HIGH risk and moving Workload$UpdatedAt

_Filed: 2026-10-25T08:58:53Z_
_Operation: UpdateAnswer | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dd185e64193b12242704f82dcc412d70cd33b9eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every writable member of `UpdateAnswerInput` (`SelectedChoices`, `ChoiceUpdates`, `Notes`, `IsApplicable`, `Reason`) is optional. `UpdateAnswer` nevertheless calls `workload.answer_mut(..)` unconditionally, which INSERTS an `AnswerRecord` for the question when none exists, and then stamps `workload.updated_at` unconditionally. So a `PATCH .../answers/{QuestionId}` with an empty body answers 200 and silently changes observable state: the question moves out of `UNANSWERED` into `HIGH` (the freshly minted record has every choice `UNSELECTED` and `selected_choices` empty, and `AnswerRecord::risk` returns `RISK_HIGH` for exactly that), the workload's `RiskCounts` roll-up shifts one question from `UNANSWERED` to `HIGH`, the question starts appearing in `ListLensReviewImprovements`, and `Workload$UpdatedAt` advances.

That contradicts the handler's own stated rule ("Each remaining member of the answer is replaced by the value the request supplies and left as stored when it supplies none") — for a question with no stored answer, an all-absent request does not leave anything as stored, it fabricates one — and it contradicts the rule the crate applies and PINS for its sibling update handlers, which guard the all-absent case and deliberately write nothing, not even `UpdatedAt`.

This is distinct from the already-FIXED UpdateAnswer issue (`20261025T060712Z-wellarchitected-updateanswer-wrong-behavior`, question resolution against the official catalog only); the fix for that is untouched here.

## Evidence

Smithy model — `smithy_query(query="operation", operation="UpdateAnswer")` input: only `WorkloadId`, `LensAlias`, `QuestionId` are `"required": true`; `SelectedChoices`, `ChoiceUpdates`, `Notes`, `IsApplicable`, `Reason` carry no `required`, so an empty body is input the model allows.

The unconditional write — services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/ops/update_answer.rs:254 and 279-299:
```
        let answer = workload.answer_mut(&input.lens_alias, &question);
...
        // Each remaining member of the answer is replaced by the value the request
        // supplies and left as stored when it supplies none, so they are written
        // independently of each other.
        if let Some(is_applicable) = input.is_applicable { ... }
        if let Some(reason) = input.reason { ... }
        if let Some(notes) = input.notes { ... }
        answer.resolve_selected_choices(&question);
        workload.updated_at = ::std::time::SystemTime::now();
```

What `answer_mut` does — src/state.rs:1874-1886:
```
    /// Recording an answer is what moves the question out of `UNANSWERED`
    /// ([`Self::risk_counts`]), so this is the one place a workload's answer comes into
    /// existence.
    pub fn answer_mut(&mut self, lens: &str, question: &ReviewQuestion) -> &mut AnswerRecord {
        self.answers
            .entry(lens_alias(lens).to_string())
            .or_default()
            .entry(question.question_id.clone())
            .or_insert_with(|| AnswerRecord::for_question(question))
    }
```
and the record it mints — src/state.rs:1251-1271 (`for_question`: every choice `CHOICE_STATUS_UNSELECTED`, `selected_choices: Vec::new()`, `is_applicable: true`), rated by src/state.rs:1325-1330:
```
    pub fn risk(&self) -> &'static str {
        if !self.is_applicable { return RISK_NOT_APPLICABLE; }
        if self.selected_choices.is_empty() { return RISK_HIGH; }
```

The rule the crate applies to its sibling update handler — src/ops/update_lens_review.rs:251-276:
```
        // Every member a request can write here is optional, so a request that supplies
        // NONE of them asks for no change: it reports the lens review as it stands and
        // leaves the workload's `UpdatedAt` where it was, rather than recording an update
        // nothing wrote.
        if input.lens_notes.is_some()
            || input.pillar_notes.is_some()
            || input.jira_configuration.is_some()
        {
            ...
            workload.updated_at = now;
        }
```
and the same guard in src/ops/update_review_template_lens_review.rs:235-250 ("a request that supplies NEITHER asks for no change").

That rule is pinned by this crate's tests — tests/all/lens_review_notes.rs:482-487:
```
    let nothing = update_lens_review(&client, &workload_id, FRAMEWORK_LENS, json!({})).await;
...
        "case \"no-member\": an update that supplies no member must change nothing, before \
```
and tests/all/review_template_lens_review.rs:374-388 ("Supplying NEITHER member writes nothing — not even `UpdatedAt`"). No test exercises a SUCCESSFUL `UpdateAnswer` with `json!({})` — tests/all/answer_updates.rs:608-638 uses an empty body only on 404 rows — so nothing pins the current behaviour.

## Steps to Reproduce

1. Boot the wellarchitected emulator
2. POST /workloads (CreateWorkload) with Lenses=["wellarchitected"]; note WorkloadId
3. GET /workloads/{WorkloadId}/lensReviews/wellarchitected/answers/ops-01 (GetAnswer): Answer.Risk is UNANSWERED
4. GET /workloads/{WorkloadId} and record Workload.UpdatedAt as T0 and RiskCounts (UNANSWERED = N, HIGH = 0)
5. PATCH /workloads/{WorkloadId}/lensReviews/wellarchitected/answers/ops-01 (UpdateAnswer) with body {} — a request that writes no member
6. Observe 200 and that the returned Answer.Risk is now HIGH with every ChoiceAnswers entry UNSELECTED
7. GET /workloads/{WorkloadId}: RiskCounts is now UNANSWERED = N-1, HIGH = 1 and UpdatedAt has moved past T0
8. GET /workloads/{WorkloadId}/lensReviews/wellarchitected/improvements (ListLensReviewImprovements): ops-01 is now an improvement item
9. Compare with PATCH /workloads/{WorkloadId}/lensReviews/wellarchitected (UpdateLensReview) with body {}, which writes nothing and leaves UpdatedAt untouched
