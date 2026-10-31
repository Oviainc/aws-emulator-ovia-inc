Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: af767dd3eb1ad507e674c736ee4881cbe95a30bf

# The ExpiresAt refusals bypass the crate's single constraint-violation wording that the location rule two lines later goes through

_Filed: 2026-10-28T14:53:07Z_
_Operation: CreateInstrumentationConfiguration | Class: error-response_

> _Note: this issue was identified against the code base at commit `af767dd3eb1ad507e674c736ee4881cbe95a30bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateInstrumentationConfiguration` enforces two conditional-member rules the model cannot express, side by side in the same handler:

1. the `ExpiresAt` rule (`crate::state::requested_expiry`) — not supported for PROBE, and within a 5-minute/24-hour window from the time of the request; and
2. the `CodeLocation.ClassName` rule (`crate::state::location_refusal` -> `code_location_refusal`) — required when `Language` is `Java`.

Rule 2 is worded through the crate's ONE constraint-violation builder, so it answers the same envelope a modelled constraint refused at the boundary answers: `1 validation error detected: Value at 'location.codeLocation.className' failed to satisfy constraint: Member must not be null for a Java code location`.

Rule 1 answers a free-form prose sentence with no `N validation error(s) detected:` envelope, no `Value at '<memberPath>' failed to satisfy constraint:` clause and no lowerCamel member path. `crate::state::constraint_clause` declares itself the single place this service words a constraint violation, explicitly including "a handler that enforces a constraint itself", and `code_location_refusal`'s own doc claims its requirement is "worded through the same clause builder as every other constraint this service states" — a claim this handler falsifies five lines earlier.

Every other handler-enforced constraint of this crate does go through the builder: `page_bound` (maxResults), `tag_count_refusal` (tags), `entity_identity_refusal` (keyAttributes), `PutGroupingConfiguration`'s repeated-grouping-name check (groupingAttributeDefinitions), `BatchGetServiceLevelObjectiveBudgetReport`'s sloIds pattern check. The one deliberate exception in the crate, `time_range_refusal`, carries an explicit justification comment for its wording; `requested_expiry` carries none. So a client that parses the `message` of a `ValidationException` to learn which member it must fix gets the member path for every input constraint of this service except this one.

The defect is in the wording of the refusal only; the status (400), the error shape (`ValidationException`) and which requests are refused are all already right.

## Evidence

THE RULE THE CRATE STATES — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:729-742:
```
/// One clause of a `ValidationException` message: the member whose value violated a
/// constraint, and the requirement it violated.
///
/// The ONE place this service words a constraint violation, reached both by the
/// framework-level refusal renderer (`crate::service`) and by a handler that enforces a
/// constraint itself, so the two cannot drift.
...
pub fn constraint_clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}' failed to satisfy constraint: {requirement}")
}
```
and src/state.rs:757-767: `pub fn validation_envelope(clauses: &[String]) -> String { ... format!("1 validation error detected: {}", clauses[0]) ... }`, which src/service.rs:176-185 (`constraint_envelope`) renders every framework-level refusal of this service through.

SIDE THAT OBEYS IT — src/state.rs:4087-4102 (`code_location_refusal`), reached from the same handler:
```
/// The path is a parameter because a refusal states the member the REQUEST carries, which is
/// `Location` for a create and `LocationIdentifier` for the reads and the delete; the
/// requirement is worded through the same clause builder as every other constraint this
/// service states.
pub fn code_location_refusal(member_path: &str, code: &CodeLocation) -> Option<String> {
    ...
    class_name_missing.then(|| {
        validation_envelope(&[constraint_clause(
            &format!("{member_path}.{CODE_LOCATION_PATH_STEP}.className"),
            &format!("Member must not be null for a {} code location", code.language),
        )])
    })
}
```

SIDE THAT BREAKS IT — src/state.rs:4226-4250 (`requested_expiry`), two free-form `Err(format!(...))` strings:
```
    if instrumentation_type != INSTRUMENTATION_TYPE_BREAKPOINT {
        return match requested {
            Some(_) => Err(format!(
                "ExpiresAt is not supported for {instrumentation_type} instrumentation \
                 configurations, which persist until they are explicitly deleted"
            )),
...
        return Err(format!(
            "ExpiresAt must be between {} and {} seconds after the time of the request for \
             {INSTRUMENTATION_TYPE_BREAKPOINT} instrumentation configurations",
```

BOTH REACHED FROM ONE HANDLER — src/ops/create_instrumentation_configuration.rs:217-234:
```
        // The model cannot express "this member is only supported for one value of that
        // one", nor a window measured from the time of the request, so both parts of the
        // documented `ExpiresAt` rule are enforced here and refused with 400 rather than
        // silently coerced.
        let expires_at = crate::state::requested_expiry(
            &input.instrumentation_type,
            input.expires_at.as_ref(),
            &now,
        )
        .map_err(crate::scaffolded::Scaffolded::validation)?;

        // Same class of rule for the location: `CodeLocation.ClassName` is required for one
        // value of its sibling `Language`, which the model cannot express either, so the
        // documented requirement is enforced here and the location is refused with 400
        // rather than hashed and stored.
        if let Some(refusal) = crate::state::location_refusal(&input.location) {
            return Err(crate::scaffolded::Scaffolded::validation(refusal));
        }
```
Note the handler's own comment calls the second rule the "Same class of rule" as the first — yet only one of them is worded by the shared builder.

THE DELIBERATE EXCEPTION, which this one is not — src/state.rs:1984-1989 (`time_range_refusal`): "the real service refuses an inverted one with `ValidationException` and states the rule with both offending instants in it. The instants are rendered the way the service states them". No such justification exists at `requested_expiry`.

TEST COVERAGE (the fix leaves it passing): tests/all/instrumentation_configurations.rs:898-962 `expires_at_follows_the_documented_window` drives every refused row (`"probe, an hour"`, `"breakpoint, one second under the floor"`, `"breakpoint, already past"`, ...) and asserts only the status code — `assert_eq!(status, expected_status, "case {label:?}: expected {expected_status}, got {status} with {answer:?}")` — never the message text, so no existing assertion pins the current wording.

## Steps to Reproduce

1. Start the emulator and point an `application-signals` client at it.
2. POST /create-instrumentation-configuration with {"InstrumentationType":"PROBE","Service":"checkout","Environment":"prod","SignalType":"SNAPSHOT","Location":{"CodeLocation":{"Language":"Java","ClassName":"CartService","FilePath":"src/CartService.java"}},"CaptureConfiguration":{"CodeCapture":{"CaptureLimits":{}}},"ExpiresAt":<now+3600>}.
3. Read the 400 body: actual message is "ExpiresAt is not supported for PROBE instrumentation configurations, which persist until they are explicitly deleted" — no `1 validation error detected:` envelope and no `Value at 'expiresAt' failed to satisfy constraint:` clause.
4. Repeat with the same body but "InstrumentationType":"BREAKPOINT" and "ExpiresAt":<now+60> (under the 5-minute floor): the message is again a bare sentence, "ExpiresAt must be between 300 and 86400 seconds after the time of the request for BREAKPOINT instrumentation configurations".
5. Contrast, in the SAME operation: POST the same body with "InstrumentationType":"BREAKPOINT", no "ExpiresAt", and "Location":{"CodeLocation":{"Language":"Java","FilePath":"src/CartService.java"}} (no ClassName) -> 400 with "1 validation error detected: Value at 'location.codeLocation.className' failed to satisfy constraint: Member must not be null for a Java code location".
6. Contrast with a modelled constraint of the same operation: send "Service":"" -> 400 with "1 validation error detected: Value at 'service' failed to satisfy constraint: Member must have length greater than or equal to 1" (src/service.rs:138-185). Two of the three refusals of one operation name the member; the ExpiresAt pair does not.
