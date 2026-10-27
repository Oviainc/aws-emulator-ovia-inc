Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3934dd8dfa9c40453bce3aa6cb9dd67d7ad9e6cc

# StartConfigurationChecks starts runs against an application whose deregistration is in flight, then loses the records it handed back

_Filed: 2026-10-25T22:53:05Z_
_Operation: StartConfigurationChecks | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3934dd8dfa9c40453bce3aa6cb9dd67d7ad9e6cc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartConfigurationChecks` resolves the application with a plain `applications.get(...)` and never looks at `application.pending`. An application whose `DeregisterApplication` is still running is still present in the map (it reports `DELETING` until the removal the clock owes it falls due), so a start against it is accepted and the handler files one `ConfigurationCheckOperation` per requested check, returning their `Id`s to the caller.

The sibling control operations of this crate refuse exactly that condition with the `ConflictException` all three of them model: `StartApplication` and `StopApplication` both bail out when `application.pending.is_some()`, with the comment "A change the clock still owes this application is an operation already under way on it, which is what `ConflictException` reports." `StartConfigurationChecks` models `ConflictException` too (it uses it for a check already in progress) but skips the pending check.

The consequence is lost state the caller was handed: when the removal falls due, `RegionState::settle` drops the application and calls `forget_configuration_checks_for(&id)`, deleting the freshly filed runs while they are still `INPROGRESS`. The `Id`s the 200 response just returned then resolve to nothing — `GetConfigurationCheckOperation` and `ListSubCheckResults` answer 400 for them — which contradicts the state module's own promise that an id handed to a caller names a record that exists.

The same ordering also makes the crate's `configuration_check_subject_gone_message` branch unreachable: `settle` removes the application and forgets its check runs before it collects the runs whose terminal status is due, so no run can ever settle with its subject missing.

## Evidence

The handler's resolution, with no pending check — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/start_configuration_checks.rs:160-174:
```
        let Some(application_id) = state
            .region(&region)
            .applications
            .get(&input.application_id)
            .map(|application| application.id.clone())
        else {
            return Err(
                StartConfigurationChecksErrorModeled::ResourceNotFoundException(
...
```
The only conflict it does raise is for a check already running — start_configuration_checks.rs:189-203.

The check its siblings apply — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/start_application.rs:166-176:
```
        // A change the clock still owes this application is an operation already under
        // way on it, which is what `ConflictException` reports.
        if application.pending.is_some() {
            return Err(StartApplicationErrorModeled::ConflictException(
```
and services/service-com-amazonaws-ssmsap-ssmsap/src/ops/stop_application.rs:189-199 (identical check and comment).

What happens to the records it filed — services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:2309-2319:
```
        for (id, arns) in completed {
            self.applications.remove(&id);
            for arn in arns {
                self.tags_by_arn.remove(&arn);
...
            self.forget_configuration_checks_for(&id);
        }
```
and the promise it breaks — services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:11-12:
```
//! So a register is observable by every later read, a deregister by every later call,
//! and an `OperationId` handed to a caller always names a record that exists.
```

The unreachable branch the same ordering creates — state.rs:2328-2346 collects the due runs AFTER the removal loop, so `None => Err(configuration_check_subject_gone_message(&application_id))` (state.rs:2345) can never be taken.

Test coverage: services/service-com-amazonaws-ssmsap-ssmsap/tests/all/configuration_checks.rs:723-776 (`deregistering_an_application_forgets_its_configuration_checks`) starts the checks BEFORE the deregistration and pins only that those runs are forgotten once the removal falls due; no test starts checks during the DELETING window, so nothing pins the behaviour described here.

## Steps to Reproduce

1. POST /register-application with {"ApplicationId":"HDB-TEST","ApplicationType":"SAP_ABAP","Instances":["i-0123456789abcdef0"]} and wait out TRANSITION_SPAN so it is ACTIVATED.
2. POST /deregister-application with {"ApplicationId":"HDB-TEST"} — 200; the application now reports DELETING with its removal pending.
3. Immediately POST /start-configuration-checks with {"ApplicationId":"HDB-TEST"} — observe HTTP 200 and a list of ConfigurationCheckOperations with fresh Ids. Compare with POST /start-application for the same application, which answers 409 ConflictException in the same window.
4. Wait out TRANSITION_SPAN, then POST /get-configuration-check-operation with one of the Ids from step 3 — observe HTTP 400 ValidationException: the record the 200 handed back no longer exists.
