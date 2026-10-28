Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# The decode-the-NextToken-or-refuse branch is hand-copied into all four paginated handlers, while its sibling page-size decision is centralized in state.rs

_Filed: 2026-10-28T13:35:34Z_
_Operation: ListChangeSets | Class: maintainability_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` deliberately owns the page-SIZE decision in one place — `page_size()` says so in its own doc: "This is the one place that decision is made, so each paginated operation judges a page size the same way". The matching page-TOKEN decision (decode the token for this list's prefix, and answer the one `invalid_page_token()` fault when it decodes to nothing) is NOT centralized, even though both halves it needs (`decode_page_token`, `invalid_page_token`) already live in state.rs. The identical nine-line `match input.next_token.as_deref() { None => 0, Some(token) => match decode_page_token(PREFIX, token) { Some(offset) => offset, None => return Err(...ValidationException(invalid_page_token()).into()) } }` branch is written out four times, once per paginated handler, differing only in the prefix constant and the error enum. The copies can drift the way only duplicated branches drift: one of them could stop refusing, or start refusing with a different fault, while the other three keep the rule. A `state::page_offset(prefix, input.next_token.as_deref())` helper returning `Result<usize, ValidationException>` — exactly the shape `page_size` already has — collapses all four call sites to one line each and changes no response.

## Evidence

The centralized sibling — services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:1332-1340:
```
/// ... This is the one
/// place that decision is made, so each paginated operation judges a page size
/// the same way ...
pub fn page_size(
```
with the two pieces the token rule needs already beside it: state.rs:2786 `pub fn decode_page_token(prefix: &str, token: &str) -> Option<usize>` and state.rs:3366 `pub fn invalid_page_token() -> ValidationException`.

Copy 1 — src/ops/list_change_sets.rs:242-255:
```
        let offset = match input.next_token.as_deref() {
            None => 0,
            Some(token) => {
                match crate::state::decode_page_token(crate::state::CHANGE_SET_PAGE_PREFIX, token) {
                    Some(offset) => offset,
                    None => {
                        return Err(ListChangeSetsErrorModeled::ValidationException(
                            crate::state::invalid_page_token(),
                        )
                        .into())
                    }
```

Copy 2 — src/ops/list_entities.rs:341-354 (same lines, `ENTITY_PAGE_PREFIX` / `ListEntitiesErrorModeled`).

Copy 3 — src/ops/list_assessments.rs:191-204 (same lines, `ASSESSMENT_PAGE_PREFIX` / `ListAssessmentsErrorModeled`).

Copy 4 — src/ops/describe_assessment.rs:220-233 (same lines, `CONTROL_PAGE_PREFIX` / `DescribeAssessmentErrorModeled`).

Tests that pin the behaviour and must stay unchanged: tests/all/request_validation.rs:309-324 ("ListChangeSets: a page token outside the token syntax" and "ListEntities: a page token this service never issued", both expecting "[Requested page token is invalid.]"), tests/all/assessments.rs:299-307 and :481 (the same fault for DescribeAssessment and ListAssessments), tests/all/change_set_lifecycle.rs:1160-1171 (a foreign NextToken refused), and tests/all/page_size_rule.rs:253-371 (the full token walk).

## Steps to Reproduce

1. Open src/ops/list_change_sets.rs:242, src/ops/list_entities.rs:341, src/ops/list_assessments.rs:191 and src/ops/describe_assessment.rs:220 side by side and observe the same decode-or-refuse branch four times, differing only in the page prefix and the error enum.
2. Compare with src/state.rs:1332-1356, where the sibling page-size decision is a single shared helper whose doc states it is "the one place that decision is made".
3. Note that both primitives the duplicated branch calls (state.rs:2786 decode_page_token, state.rs:3366 invalid_page_token) are already public in state.rs, so only the branch itself is duplicated.
4. Confirm the behaviour is already pinned by request_validation.rs:309-324 and assessments.rs:299-307/:481, so folding the branch into one state.rs helper must leave every response and every test unchanged.
