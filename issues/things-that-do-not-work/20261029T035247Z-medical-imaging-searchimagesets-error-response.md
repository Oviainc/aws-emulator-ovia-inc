Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 50acf5c50341569d447ddf04c6c32ab03f586e64

# SearchImageSets names a filter's position with a 0-based bracket index, against the crate's own 1-based `<member>.<N>.member` rule for a list element

_Filed: 2026-10-29T03:52:47Z_
_Operation: SearchImageSets | Class: error-response_

> _Note: this issue was identified against the code base at commit `50acf5c50341569d447ddf04c6c32ab03f586e64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every refusal this service raises itself goes through one envelope (`crate::state::handler_requirement_error`), and the crate states ONE rule for how a refusal names a LIST ELEMENT's position: `<member>.<1-based index>.member`. `UntagResource` applies that rule (`tagKeys.1.member`, `tagKeys.2.member`, ...) and its suite pins it. `SearchImageSets` is the only other handler that has to name an element of a list, and it spells the same thing two ways differently: a bracketed index (`searchCriteria.filters[0].values`) and a ZERO-based one, because the index comes straight from `.enumerate()` with no `+ 1`.

Consequence: one service answers two conventions, and the position it reports is off by one against its own rule — a request whose SECOND filter cannot be evaluated is refused with `searchCriteria.filters[1].values`, which under the convention the same crate uses for `tagKeys` names the FIRST element. A client that reads the failing position out of the `ValidationException` message is pointed at the wrong filter. Both of the SearchImageSets paths are affected (`...values` and `...operator`), and the message is the only thing wrong: the status (400) and the code (`ValidationException`) are correct.

## Evidence

The rule, stated and applied by the sibling handler — services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/untag_resource.rs:170-176:
```
        // Each key named for removal is a `TagKey` too, so it answers to the same
        // modelled constraints — ... A list element is reported at the position the framework spells
        // it with, `<member>.<1-based index>.member`.
        for (position, key) in input.tag_keys.iter().enumerate() {
            let path = format!("tagKeys.{}.member", position + 1);
```
The same rule restated by its suite, which pins the wire text — tests/all/untag_key_validation.rs:12-13 ("it names the failing element's path (`tagKeys.<1-based index>.member`)"), tests/all/untag_key_validation.rs:83 (`assert_refused(row.case, &refused, "tagKeys.1.member")`) and :126 (`&format!("tagKeys.{}.member", index + 1)`).

The other spelling, bracketed and 0-based — src/ops/search_image_sets.rs:572-574:
```
    /// Read the filter at `index` of the criteria, or state why it cannot be evaluated.
    fn from_filter(index: usize, filter: &SearchFilter) -> Result<Self, String> {
        let values_path = format!("searchCriteria.filters[{index}].values");
```
and src/ops/search_image_sets.rs:613-617:
```
            other => Err(crate::state::handler_requirement_error(
                &format!("searchCriteria.filters[{index}].operator"),
                Some(other),
                "Member must satisfy enum value set [EQUAL, BETWEEN]",
            )),
```
The index is the raw 0-based one — src/ops/search_image_sets.rs:193-195:
```
            .iter()
            .enumerate()
            .map(|(index, filter)| Predicate::from_filter(index, filter))
```
Both handlers word the clause through the one envelope, so nothing but the path differs — src/state.rs:589-596:
```
pub fn handler_requirement_error(path: &str, value: Option<&str>, requirement: &str) -> String {
    let clause = match value { ... _ => format!("{path}{CONSTRAINT_SEPARATOR}{requirement}"), };
    format!("1 validation error detected: {clause}")
```
Test coverage: the SearchImageSets refusal rows assert only status and code, never the message path (tests/all/image_set_search.rs:871-1046, every row carries `expected_status: 400, expected_code: "ValidationException"` and no expected path), while the UntagResource rows assert the path text itself — so the inconsistent spelling is pinned by nothing and the 1-based rule is pinned by tests.

## Steps to Reproduce

1. Create a data store and let the clock settle it to ACTIVE.
2. POST /datastore/{datastoreId}/searchImageSets with a `searchCriteria` payload carrying TWO filters, the first valid (e.g. `{"operator":"EQUAL","values":[{"DICOMPatientId":"PID-1"}]}`) and the second unevaluable (e.g. `{"operator":"BETWEEN","values":[{"DICOMPatientId":"PID-1"},{"DICOMPatientId":"PID-2"}]}`).
3. Observe HTTP 400 `ValidationException` with `1 validation error detected: searchCriteria.filters[1].values failed to satisfy constraint: ...` — a bracketed, 0-based position for the SECOND filter.
4. DELETE /tags/{resourceArn}?tagKeys=ok&tagKeys=aws:reserved on the same data store and observe the sibling refusal for the second list element: `1 validation error detected: Value 'aws:reserved' at 'tagKeys.2.member' failed to satisfy constraint: ...` — the crate's stated `<member>.<1-based index>.member` spelling.
5. Compare: one service reported element #2 as `[1]` on one route and as `2` on the other.
