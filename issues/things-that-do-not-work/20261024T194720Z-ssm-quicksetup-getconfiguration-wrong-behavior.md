Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7cd33b8d616838a49131331fab4e1704473d18dc

# Status summaries only ever report a two-state Deployment status, never DEPLOYING and never an AsyncExecutions summary

_Filed: 2026-10-24T19:47:20Z_
_Operation: GetConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7cd33b8d616838a49131331fab4e1704473d18dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every status summary this emulator can produce comes from one constructor, `crate::state::StatusSummaryRecord::deployment()`, which builds exactly one summary whose `StatusType` is `Deployment`, whose `Status` is `INITIALIZING` and which settles straight to `SUCCEEDED` after `TRANSITION_SPAN`, with `StatusMessage`/`StatusDetails` always empty. As a result:

* a configuration read with `GetConfiguration` (and likewise the summaries in `GetConfigurationManager`, `ListConfigurationManagers` and `ListConfigurations`) carries a single-element `StatusSummaries` list — the modelled `AsyncExecutions` status type is unreachable, so a caller that inspects the non-deployment phase of a Quick Setup rollout finds nothing;
* the deployment itself jumps `INITIALIZING -> SUCCEEDED`, so the modelled intermediate `DEPLOYING` is never observable even though a Quick Setup deployment of stacks/associations across targeted accounts and Regions spends its time there;
* `StatusMessage` and `StatusDetails` are never populated, although `StatusSummary$StatusMessage` is documented as the informational message "relevant to the current status and status type" and is what the real service carries on the non-deployment summary.

Expected: a configuration (and its manager) reports one summary per modelled `StatusType` — a `Deployment` summary that passes through `DEPLOYING` before settling, plus an `AsyncExecutions` summary (e.g. `NONE`/`SUCCEEDED` with a `StatusMessage`).
Actual: one `Deployment` summary with a two-value state machine, no `StatusMessage`, no `StatusDetails`, and no second status type at any point of the lifecycle.

## Evidence

Spec grounding: `com.amazonaws.ssmquicksetup#StatusType` publishes exactly two values, `Deployment` and `AsyncExecutions` (smithy_query shape StatusType), and `com.amazonaws.ssmquicksetup#Status` publishes `INITIALIZING, DEPLOYING, SUCCEEDED, DELETING, STOPPING, FAILED, STOPPED, DELETE_FAILED, STOP_FAILED, NONE` — a set that exists for observable phases. `ConfigurationSummary$StatusSummaries` is documented as "A summary of the state of the configuration manager. This includes deployment statuses, association statuses, drift statuses, health checks, and more", i.e. more than one kind of summary; `GetConfigurationOutput$StatusSummaries` targets the same `StatusSummariesList`.

Handler/state evidence: `src/ops/get_configuration.rs:204-210` serializes `configuration.status_summaries` verbatim; `src/state.rs` `StatusSummaryRecord::deployment()` is the ONLY constructor (`status_type: STATUS_TYPE_DEPLOYMENT`, `status: STATUS_INITIALIZING`, `status_message: None`, `status_details: BTreeMap::new()`, settling to `STATUS_SUCCEEDED`), and it is the only thing `create_configuration_manager.rs:301,328` and `state::restart_deployment` (used by `update_configuration_definition.rs:294,309`) ever push. `STATUS_TYPE_VALUES` in `src/ops/types/shared_types.rs:72` shows `AsyncExecutions` is modelled but never constructed anywhere in the crate (grep for `AsyncExecutions` matches only the generated type list).

Test coverage: the current two-state model is pinned by `tests/all/configuration_managers.rs:1384` (`the_deployment_status_settles_once_the_clock_owes_it`) and `:1743` (`reading_a_configuration_reports_the_status_the_clock_owes_and_moves_nothing`), which assert exactly one summary of type `Deployment` going `INITIALIZING -> SUCCEEDED`; those tests would need updating. No test exercises `DEPLOYING`, `AsyncExecutions`, `StatusMessage` or `StatusDetails`. (Distinct from the already-open DeleteConfigurationManager issue, which covers only the `DELETING`/`DELETE_FAILED` teardown phase.)

## Steps to Reproduce

1. POST /configurationManager with {"Name":"statuses","ConfigurationDefinitions":[{"Type":"AWSQuickSetupType-SSMHostMgmt","Parameters":{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-west-2"}}]} and keep the returned ManagerArn.
2. POST /listConfigurations with {"ManagerArn":"<ManagerArn>"} to learn the configuration Id.
3. GET /getConfiguration/<Id> immediately: StatusSummaries holds a single entry {StatusType: "Deployment", Status: "INITIALIZING"} — no AsyncExecutions entry, no StatusMessage.
4. Wait past the emulator's transition span and GET /getConfiguration/<Id> again: the one entry flips to SUCCEEDED; DEPLOYING is never reported, and the second modelled status type never appears in this or any other response (GetConfigurationManager, ListConfigurationManagers, ListConfigurations).
