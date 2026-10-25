Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# Delete removes the manager and its configurations synchronously, so the modeled DELETING status is never observable

_Filed: 2026-10-24T15:16:32Z_
_Operation: DeleteConfigurationManager | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Deleting a Quick Setup configuration manager is an asynchronous teardown: the service accepts the request, reports the manager's `Deployment` status summary as `DELETING` (and `DELETE_FAILED` when the teardown fails) while it removes the deployed resources, and only then stops returning the resource. The `Status` enum the model publishes exists for exactly those observable phases (`INITIALIZING`, `DEPLOYING`, `SUCCEEDED`, `DELETING`, `STOPPING`, `FAILED`, `STOPPED`, `DELETE_FAILED`, `STOP_FAILED`, `NONE`).

The emulator's handler takes the store lock and removes the record outright (`state.lock().remove_manager(&input.manager_arn)`), which also drops every configuration the manager owned in the same instant. No status is ever written, so `DELETING` and `DELETE_FAILED` are unreachable states in this emulator and a caller that polls after a delete sees the resource blink out of existence rather than wind down.

Expected: `DELETE /configurationManager/{ManagerArn}` answers 200 and the manager remains readable for the in-flight teardown with its `Deployment` status summary reporting `DELETING` (and its configurations likewise), after which it is gone; polling with `GetConfigurationManager` is the documented way a client observes completion.
Actual: the record is gone before the response is written — an immediately following `GetConfigurationManager` is a 404 and `ListConfigurations` reports nothing — so no client-visible delete lifecycle exists. Note the emulator already models exactly this kind of clock-owed transition for the create/update deployment (`crate::state::StatusSummaryRecord::deployment` / `settle`, `TRANSITION_SPAN`), so the deletion phase is the one lifecycle left unmodelled.

## Evidence

Spec grounding: `com.amazonaws.ssmquicksetup#Status` publishes `DELETING` and `DELETE_FAILED` (smithy_query shape Status), and `GetConfigurationManagerOutput$StatusSummaries` is documented as "Summaries of the state of the configuration manager ... an aggregate of the statuses from the configuration definition associated with the configuration manager" — states a synchronous delete can never produce. Well-established service behavior: Quick Setup's delete tears down the deployed stacks/associations across the targeted accounts and Regions, so it cannot complete within the API call.

Handler evidence: `src/ops/delete_configuration_manager.rs:180-192` — the whole implementation is `state.lock().remove_manager(&input.manager_arn)` plus the 404 branch; `src/state.rs` `remove_manager` removes the manager and retains-out every configuration whose `manager_arn` matches, with no status write and no pending transition (`StatusSummaryRecord::settles`).

Test coverage: `tests/all/configuration_managers.rs:1196` (`a_delete_removes_the_manager_and_the_configurations_it_owned`) asserts the immediate-404 behavior, so the synchronous semantics are currently pinned by a test that would need updating; no test exercises a DELETING status.

## Steps to Reproduce

1. POST /configurationManager with {"Name":"doomed","ConfigurationDefinitions":[{"Type":"AWSQuickSetupType-SSMHostMgmt","Parameters":{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-west-2"}}]} and keep the returned ManagerArn.
2. DELETE /configurationManager/{ManagerArn} — answers 200.
3. Immediately GET /configurationManager/{ManagerArn}: the emulator answers 404 ResourceNotFoundException. Expected: the manager is still reported with its Deployment StatusSummary carrying Status DELETING while the teardown is in flight.
4. POST /listConfigurations with {"ManagerArn":"<ManagerArn>"}: the emulator answers 404; no phase of the deletion is observable, and the modeled DELETING / DELETE_FAILED statuses can never appear in any response.
