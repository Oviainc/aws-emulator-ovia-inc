Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 43848a0bc39d081d66f97fb98e998d8aaf0170b2

# The TagLimitExceeded -> ValidationException mapping is built inline in two handlers instead of in state.rs beside the crate's other shared refusals

_Filed: 2026-11-06T02:37:41Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `43848a0bc39d081d66f97fb98e998d8aaf0170b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The per-report tag-limit refusal is the same rule in two places: `PutReportDefinition` and `TagResource` both call into `StoredReport::merge_tags`, both match on `crate::state::TagLimitExceeded { requested, limit }`, and both hand-build a `ValidationException` whose message names the limit and the resulting count. The two copies have already drifted in wording (`the request supplies {requested}.` vs `the request would leave it holding {requested}.`), which is exactly the drift this crate avoids elsewhere by hosting each shared refusal in `src/state.rs` as a single constructor (`report_not_found`, `report_to_update_not_found`, `PARQUET_PREFERENCE_MESSAGE`) — `src/state.rs:46-47` states that rationale in so many words. Moving the mapping into `state.rs` (one function per phrasing, or one taking the clause so both texts stay byte-identical) keeps every response and every existing test unchanged while leaving one place where the rule and its wording live.

## Evidence

Copy 1 — `src/ops/put_report_definition.rs:206-219`:
```
        let report = match crate::state::StoredReport::new(input.report_definition, &tags) {
            Ok(report) => report,
            Err(crate::state::TagLimitExceeded { requested, limit }) => {
                return Err(PutReportDefinitionErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(format!(
                            "Tags: a report definition can hold at most {limit} tags; the \
                             request supplies {requested}."
```
Copy 2 — `src/ops/tag_resource.rs:165-174`:
```
        if let Err(crate::state::TagLimitExceeded { requested, limit }) =
            report.merge_tags(&input.tags)
        {
            return Err(TagResourceErrorModeled::ValidationException(
                ValidationException {
                    message: Some(format!(
                        "Tags: a report definition can hold at most {limit} tags; the \
                         request would leave it holding {requested}."
```
The crate's own convention for a refusal answered by more than one handler — `src/state.rs:46-47`:
```
/// request (least of all the report name) is echoed back in it. One constructor so the
/// handlers that answer this refusal cannot drift apart.
pub fn report_not_found() -> ResourceNotFoundException {
```

Tests that cover both paths and would stay green: `tests/all/modelled_constraints.rs:399-412` (`the_tag_set_of_a_report_is_bounded_however_often_it_is_tagged`), `tests/all/modelled_constraints.rs:256-262` (`tag_input_outside_its_modelled_constraint_is_refused`, exercised "through both operations that take tags"), and `tests/all/refusal_envelope.rs:296-343` (`an_input_constraint_refusal_still_reports_which_member_is_wrong`, which asserts only a 400 `ValidationException` with a non-empty message).

## Steps to Reproduce

1. Read `src/ops/put_report_definition.rs:201-219` and `src/ops/tag_resource.rs:162-177`: the same `TagLimitExceeded` branch and the same refusal sentence, written twice with two different second clauses.
2. Read `src/state.rs:38-50` and `src/state.rs:134-140`: every other refusal shared by more than one handler of this crate is constructed once in `state.rs`, with the stated reason that the handlers "cannot drift apart".
3. Change the per-report tag-limit wording and note that two files must be edited in lockstep for the two paths to keep answering the same rule.
