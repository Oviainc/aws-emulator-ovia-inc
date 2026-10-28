Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: af767dd3eb1ad507e674c736ee4881cbe95a30bf

# The StartTime/EndTime pairing refusals are free-form sentences, not the crate's single constraint-violation wording used by the location rule beside them

_Filed: 2026-10-28T14:53:07Z_
_Operation: GetInstrumentationConfigurationStatus | Class: error-response_

> _Note: this issue was identified against the code base at commit `af767dd3eb1ad507e674c736ee4881cbe95a30bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetInstrumentationConfigurationStatus` enforces two rules the model cannot express, one after the other in the same handler:

1. the documented `StartTime`/`EndTime` pairing (`crate::state::status_event_window`) — both or neither, and a window that does not end before it begins; and
2. the `CodeLocation.ClassName` rule (`crate::state::identified_location_refusal` -> `code_location_refusal`).

Rule 2 goes through the crate's one constraint-violation builder and answers `1 validation error detected: Value at 'locationIdentifier.codeLocation.className' failed to satisfy constraint: Member must not be null for a Java code location`. Rule 1 answers two bare sentences — `"StartTime and EndTime must both be provided together or both be omitted"` and `"EndTime must not be earlier than StartTime"` — with no `N validation error(s) detected:` envelope, no `Value at '<memberPath>' failed to satisfy constraint:` clause and no member path at all for the inverted-window case.

`crate::state::constraint_clause` declares itself the ONE place this service words a constraint violation, "reached both by the framework-level refusal renderer (`crate::service`) and by a handler that enforces a constraint itself, so the two cannot drift". The sibling paginated read of this same crate holds to that even for a rule its own model leaves unstated: `ListServiceStates` refuses its page bound through `crate::state::page_bound` with a dedicated `MAX_RESULTS_MEMBER_PATH` constant whose comment insists the member path is "spelled the way the boundary validator spells a member path for this service". This operation's window rule is the one input constraint of the crate that states neither the member nor the envelope.

As in the create, only the wording is wrong: the status (400), the shape (`ValidationException`) and which requests are refused are already correct.

## Evidence

THE RULE THE CRATE STATES — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:729-742:
```
/// The ONE place this service words a constraint violation, reached both by the
/// framework-level refusal renderer (`crate::service`) and by a handler that enforces a
/// constraint itself, so the two cannot drift.
...
pub fn constraint_clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}' failed to satisfy constraint: {requirement}")
}
```

SIDE THAT BREAKS IT — src/state.rs:3662-3681 (`status_event_window`):
```
    match (start, end) {
        (None, None) => Ok((...)),
        (Some(start), Some(end))
            if status_event_instant(end) >= status_event_instant(start) =>
        {
            Ok((*start, *end))
        }
        (Some(_), Some(_)) => Err("EndTime must not be earlier than StartTime".to_string()),
        _ => Err(
            "StartTime and EndTime must both be provided together or both be omitted".to_string(),
        ),
    }
```

SIDE THAT OBEYS IT, in the SAME handler eight lines later — src/ops/get_instrumentation_configuration_status.rs:198-215:
```
        // The model cannot express "both of these members or neither", so the documented
        // pairing is enforced here and refused with 400 rather than read as half a window.
        let requested_window = crate::state::status_event_window(
            input.start_time.as_ref(),
            input.end_time.as_ref(),
            &now,
        )
        .map_err(crate::scaffolded::Scaffolded::validation)?;

        // The same class of rule on the location: `CodeLocation.ClassName` is required for
        // one value of its sibling `Language`, so the documented requirement is enforced
        // here too and refused with 400, as it is on the create that would have stored such
        // a location.
        if let Some(refusal) =
            crate::state::identified_location_refusal(&input.location_identifier)
        {
            return Err(crate::scaffolded::Scaffolded::validation(refusal));
        }
```
(the handler itself calls the second "The same class of rule"), with `code_location_refusal` at src/state.rs:4094-4101 building `validation_envelope(&[constraint_clause(...)])`.

SIBLING PAGINATED READ THAT DOES WORD ITS OWN HANDLER-ENFORCED RULE THROUGH THE BUILDER — src/ops/list_service_states.rs:195-200 and :269-272:
```
        let page_size = crate::state::page_bound(
            MAX_RESULTS_MEMBER_PATH,
            input.max_results,
            crate::state::MAX_SERVICE_STATES_PAGE,
        )
        .map_err(refused)?;
...
/// The member path a refusal of this operation's page bound names: `MaxResults`, spelled the
/// way the boundary validator spells a member path for this service — camel case, as the
/// refusal of the same bound on a sibling list spells it.
const MAX_RESULTS_MEMBER_PATH: &str = "maxResults";
```
and `page_bound` at src/state.rs:1378-1391 returns `validation_envelope(&[constraint_clause(member_path, ...)])`.

TEST COVERAGE (the fix leaves it passing): tests/all/instrumentation_configuration_status.rs:1091-1131 `a_half_stated_window_is_refused` drives all three refused rows ("only StartTime", "only EndTime", "a window that ends before it begins") and asserts only `assert_eq!(code, 400, ...)` and `assert_eq!(error["__type"].as_str(), Some("ValidationException"), ...)` — the message text is not pinned.

## Steps to Reproduce

1. Start the emulator and create a configuration: POST /create-instrumentation-configuration with {"InstrumentationType":"PROBE","Service":"checkout","Environment":"prod","SignalType":"SNAPSHOT","Location":{"CodeLocation":{"Language":"Java","ClassName":"Cart","FilePath":"src/Cart.java"}},"CaptureConfiguration":{"CodeCapture":{"CaptureLimits":{}}}} and keep the returned LocationHash.
2. POST /get-instrumentation-configuration-status with {"InstrumentationType":"PROBE","Service":"checkout","Environment":"prod","SignalType":"SNAPSHOT","LocationIdentifier":{"LocationHash":"<hash>"},"StartTime":<now-60>} (EndTime omitted).
3. Read the 400 body: actual message is the bare "StartTime and EndTime must both be provided together or both be omitted" — no `1 validation error detected:` envelope and no `Value at 'startTime'/'endTime' failed to satisfy constraint:` clause.
4. Repeat with {"StartTime":<now>,"EndTime":<now-60>}: the message is the bare "EndTime must not be earlier than StartTime", which names no member path at all.
5. Contrast, same operation: send "LocationIdentifier":{"CodeLocation":{"Language":"Java","FilePath":"src/Cart.java"}} (no ClassName) -> 400 with "1 validation error detected: Value at 'locationIdentifier.codeLocation.className' failed to satisfy constraint: Member must not be null for a Java code location".
6. Contrast with the sibling paginated read: POST /service/states with {"StartTime":<now-3600>,"EndTime":<now>,"MaxResults":0} -> 400 with "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1".
