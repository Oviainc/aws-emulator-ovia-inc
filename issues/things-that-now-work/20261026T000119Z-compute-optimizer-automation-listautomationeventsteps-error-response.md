Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 22964030325ab1e634e5ef0028a831687ff48548

# ListAutomationEventSteps' nextToken refusal echoes the caller's token and omits the "1 validation error detected" prefix produced by constraint_report

_Filed: 2026-10-26T00:01:19Z_
_Operation: ListAutomationEventSteps | Class: error-response_

> _Note: this issue was identified against the code base at commit `22964030325ab1e634e5ef0028a831687ff48548`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both `nextToken` refusals in this handler are built with an inline `format!`/literal instead of `crate::state::constraint_report`, so the first one interpolates the caller-supplied token into the error body and neither carries the "1 validation error detected: " prefix the crate's single validation reporter produces. Every other paginated list of this service refuses an unknown token through `constraint_report` (e.g. `ListAutomationRules`, `ListRecommendedActions`, `ListAutomationEventSummaries`, both preview lists), and `state.rs` states the no-echo rule explicitly. The token path is reachable: `NextToken`'s pattern admits any `[A-Za-z0-9+/=]+` string, so a token this service never issued reaches the handler and is replayed back in the message.

## Evidence

The rule the crate states for its refusals — `src/state.rs:580-583`:
```
/// does NOT echo the rejected value — a request value (a name, or a secret on
/// some other member) must not be replayed into an error body
```
and the reporter that implements it, `src/state.rs:621-622`:
```
pub fn constraint_report(member: &str, requirement: &str) -> String {
    format!("1 validation error detected: Value at '{member}' {CONSTRAINT_MARKER}{requirement}")
```

This handler — `src/ops/list_automation_event_steps.rs:182-195`:
```
let Some(cursor) = crate::state::decode_page_token(token) else {
    return Err(invalid_parameter(format!(
        "Value '{token}' at 'nextToken' failed to satisfy constraint: Member must \
         be a token returned by a previous ListAutomationEventSteps call"
    )));
};
if cursor.filter_fingerprint != query_fingerprint {
    return Err(invalid_parameter(
        "Value at 'nextToken' failed to satisfy constraint: Member must be a \
         token returned for the same eventId"
            .to_string(),
    ));
```

The sibling path for the identical situation — `src/ops/list_automation_rules.rs:183-195`:
```
let Some(cursor) = crate::state::decode_page_token(token) else {
    return Err(invalid_parameter(crate::state::constraint_report(
        "nextToken",
        "Member must be a token returned by a previous ListAutomationRules call",
    )));
};
if cursor.filter_fingerprint != filter_fingerprint {
    return Err(invalid_parameter(crate::state::constraint_report(
        "nextToken",
        "Member must be a token returned for the same filters",
    )));
```

Test coverage: `tests/all/automation_event_steps.rs:523-539` sends `{"eventId": ..., "nextToken": "notatokenweissued"}` and asserts only `400` + `InvalidParameterValueException` via `assert_refused`, so the message text is not pinned; `tests/all/error_payloads.rs:411-419` + `:490-495` pin the prefixed, non-echoing form for the same member on a sibling operation.

## Steps to Reproduce

1. Enroll the caller, then `StartAutomationEvent` on a recommended action to obtain an `eventId`.
2. Call `ListAutomationEventSteps` with `{"eventId": "<that id>", "nextToken": "notatokenweissued"}`.
3. Observed message: `Value 'notatokenweissued' at 'nextToken' failed to satisfy constraint: Member must be a token returned by a previous ListAutomationEventSteps call` — the caller's token is echoed and the `1 validation error detected: ` prefix is missing.
4. Call `ListAutomationRules` with `{"nextToken": "notatokenweissued"}` for the contrast: `1 validation error detected: Value at 'nextToken' failed to satisfy constraint: Member must be a token returned by a previous ListAutomationRules call`.
