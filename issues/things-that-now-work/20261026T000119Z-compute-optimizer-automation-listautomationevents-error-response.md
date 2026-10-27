Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 22964030325ab1e634e5ef0028a831687ff48548

# ListAutomationEvents hand-builds its 400 messages, echoing the caller's value and dropping the "1 validation error detected" prefix every sibling uses

_Filed: 2026-10-26T00:01:19Z_
_Operation: ListAutomationEvents | Class: error-response_

> _Note: this issue was identified against the code base at commit `22964030325ab1e634e5ef0028a831687ff48548`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other handler in this crate words a handler-side refusal through `crate::state::constraint_report`, which prefixes "1 validation error detected: " and deliberately does NOT echo the rejected request value. `ListAutomationEvents` is the exception: all three of its `InvalidParameterValueException` messages are built with an inline `format!`, two of them interpolating the caller-supplied value (the filter name, and the raw `nextToken`), and none of them carrying the count prefix the rest of the service (and the crate's own test for the equivalent refusal) produces.

The divergence is reachable on the `nextToken` path: `NextToken`'s pattern `^[A-Za-z0-9+/=]+$` admits a token this service never issued, so `decode_page_token` returns `None` and the handler answers with the caller's token replayed inside the error body. The result is that the SAME refusal differs by operation: `ListAutomationEventSummaries`, which the crate says gives "the same refusal `ListAutomationEvents` gives for the same filter list", answers through `constraint_report` with no echo and with the prefix, while `ListAutomationEvents` answers with neither.

Fix is to route all three refusals through `crate::state::constraint_report` (and the existing `crate::state::unknown_event_filter_name` report), exactly as the sibling does; the existing tests assert only status 400 and the error code, so they stay green.

## Evidence

The crate's stated rule, in its one validation reporter — `src/state.rs:580-583`:
```
/// does NOT echo the rejected value — a request value (a name, or a secret on
/// some other member) must not be replayed into an error body — and reports a
/// `@pattern` as the regular expression itself, without the anchors the model
/// spells it with.
```
and `src/state.rs:621-622`:
```
pub fn constraint_report(member: &str, requirement: &str) -> String {
    format!("1 validation error detected: Value at '{member}' {CONSTRAINT_MARKER}{requirement}")
```

What this handler does instead — `src/ops/list_automation_events.rs:174-180`:
```
if let Some(unknown) = crate::state::unknown_event_filter_name(&filters) {
    return Err(invalid_parameter(format!(
        "Value '{unknown}' at 'filters.member.name' failed to satisfy constraint: Member \
         must satisfy enum value set: [{}]",
        AUTOMATION_EVENT_FILTER_NAME_VALUES.join(", ")
    )));
```
and `src/ops/list_automation_events.rs:206-218`:
```
let Some(cursor) = crate::state::decode_page_token(token) else {
    return Err(invalid_parameter(format!(
        "Value '{token}' at 'nextToken' failed to satisfy constraint: Member must \
         be a token returned by a previous ListAutomationEvents call"
    )));
};
if cursor.filter_fingerprint != query_fingerprint {
    return Err(invalid_parameter(
        "Value at 'nextToken' failed to satisfy constraint: Member must be a \
         token returned for the same filters and time range"
            .to_string(),
    ));
```

The sibling over the same events and the same filter list — `src/ops/list_automation_event_summaries.rs:197-210`:
```
// `AutomationEventFilter$name` is typed as the `AutomationEventFilterName`
// value set, but a name outside it is refused here too so a filter is
// never silently ignored — the same refusal `ListAutomationEvents` gives
// for the same filter list, since both operations narrow the same events
// with it.
if crate::state::unknown_event_filter_name(&filters).is_some() {
    return Err(invalid_parameter(crate::state::constraint_report(
        "filters.member.name",
```
and its token refusals at `src/ops/list_automation_event_summaries.rs:265-277`, both through `constraint_report`.

Test coverage: `tests/all/error_payloads.rs:411-419` pins the service's refusal format for a `nextToken` and forbids the echo — `"1 validation error detected: Value at 'nextToken' failed to satisfy constraint: ..."` with `vec!["not a token!".to_string()]` in the must-not-contain list, asserted by `tests/all/error_payloads.rs:490-495`:
```
assert!(
    !message.contains(value),
    "case {case:?}: the report must not echo the rejected value, got {message:?}"
);
```
`tests/all/automation_events.rs:740-759` exercises the ListAutomationEvents "foreign-token" path but asserts only the 400 and the error code, so it does not pin the diverging text.

## Steps to Reproduce

1. Enroll the caller (`UpdateEnrollmentConfiguration` with status=Active).
2. Call `ListAutomationEvents` with `{"nextToken": "notatokenweissued"}` and read the error `message`: it is `Value 'notatokenweissued' at 'nextToken' failed to satisfy constraint: Member must be a token returned by a previous ListAutomationEvents call` — the caller's token is echoed and the `1 validation error detected: ` prefix is absent.
3. Call `ListAutomationEventSummaries` with the same `{"nextToken": "notatokenweissued"}`: the message is `1 validation error detected: Value at 'nextToken' failed to satisfy constraint: Member must be a token returned by a previous ListAutomationEventSummaries call` — prefixed and with no echo.
4. Compare with `tests/all/error_payloads.rs:411-419`, which pins the second form for this service.
