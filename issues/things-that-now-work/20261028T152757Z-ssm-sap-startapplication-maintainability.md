Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: ea262c1f0c770608da2cb6c8011fa8bda70e1973

# The "a change the clock still owes this application is an operation in progress" conflict gate is hand-copied into five handlers

_Filed: 2026-10-28T15:27:57Z_
_Operation: StartApplication | Class: maintainability_

> _Note: this issue was identified against the code base at commit `ea262c1f0c770608da2cb6c8011fa8bda70e1973`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

One rule — "`application.pending.is_some()` means work is already under way on this application, which is what `ConflictException` reports" — is implemented five times, once per handler that controls an application, each with its message formatted inline at the call site. Three of the five share a verbatim template differing only in the verb.

This crate's own convention is the opposite: every other per-condition refusal has exactly one construction site in `state.rs`, and `application_not_found_message` states why — "every handler that acts on or reads under the application a request names reports the SAME template around it — which is what a client surfacing or matching on the service's not-found wording reads". `no_registered_application`, `no_component_message`, `no_operation_message`, `resource_not_found_message`, `conditional_requirement_message` and `settings_update_message` all live there; this one rule does not, so the five copies can drift in wording (two already have) or in the condition itself — a sixth control path added later has nothing to call.

A single `state.rs` helper taking the application and the verb phrase, returning the `ConflictException` each handler then lifts into its own modelled error variant, reproduces all five messages byte for byte. Responses and tests are unaffected: the crate's conflict tests assert HTTP 409 and the `ConflictException` error code only, never the message text.

## Evidence

Copy 1, `services/service-com-amazonaws-ssmsap-ssmsap/src/ops/start_application.rs:166-176`:
```rust
        // A change the clock still owes this application is an operation already under
        // way on it, which is what `ConflictException` reports.
        if application.pending.is_some() {
            return Err(StartApplicationErrorModeled::ConflictException(
                crate::state::conflict(format!(
                    "Application {} is {} and cannot be started until that completes",
                    application.id, application.status
                )),
            )
            .into());
        }
```
Copy 2, `src/ops/stop_application.rs:226-236` — the same template, verb changed:
```rust
    if application.pending.is_some() {
        return Err(StopApplicationErrorModeled::ConflictException(
            crate::state::conflict(format!(
                "Application {} is {} and cannot be stopped until that completes",
                application.id, application.status
            )),
        )
```
Copy 3, `src/ops/start_application_refresh.rs:173-183`: `"Application {} is {} and cannot be refreshed until that completes"`.
Copy 4, `src/ops/update_application_settings.rs:211-220`: `"Application {} has an operation in progress and its settings cannot be updated until that operation completes"`.
Copy 5, `src/ops/start_configuration_checks.rs:187-196`: `"Application {} has an operation in progress and configuration checks cannot be started until that operation completes"`.

The crate's own stated rule for exactly this kind of refusal, `src/state.rs:3852-3864`:
```rust
/// The message a call reports when the application it names is not registered in this
/// account and Region.
///
/// Built from the request: ... so every handler that acts on or reads under the application
/// a request names reports the SAME template around it ...
pub fn application_not_found_message(reference: &str) -> String {
    format!("Application {reference} doesn't exist.")
}
```

Tests that cover the five copies, and which a shared helper leaves unchanged because they assert status and code only: `tests/all/application_lifecycle.rs:1195-1201` (`busy.status, 409` + `"ConflictException"`), `tests/all/application_refresh.rs:358` and `:615` (`(409, CONFLICT_EXCEPTION)`), `tests/all/update_application_settings.rs:811` and `:837`, `tests/all/configuration_checks_in_flight.rs:262-284`, `tests/all/deregistration_window.rs:355`.

## Steps to Reproduce

1. Open src/ops/start_application.rs:166-176, src/ops/stop_application.rs:226-236 and src/ops/start_application_refresh.rs:173-183 side by side: the same `if application.pending.is_some()` branch and the same message template, copied three times
2. Open src/ops/update_application_settings.rs:211-220 and src/ops/start_configuration_checks.rs:187-196: the same condition again, with a second wording of the same sentence
3. Compare with src/state.rs:3763 (`no_registered_application`), :3778 (`no_component_message`), :3862 (`application_not_found_message`), :3873 (`resource_not_found_message`): every other refusal of this service has one construction site in state.rs, and :3852-3861 states that as the rule
4. Grep the test suite for the conflict messages (`cannot be started until`, `has an operation in progress`): no match — the tests assert only HTTP 409 and the ConflictException code, so folding the five copies into one state.rs helper changes no response and no test
