Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# CreateExperimentTemplate accepts templates FIS refuses: no target resourceArns/resourceTags exclusivity, no action->target reference check, no selectionMode form check, unknown actionId and missing required action parameters all persisted

_Filed: 2026-10-24T00:04:50Z_
_Operation: CreateExperimentTemplate | Class: validation_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler validates only four things beyond the modelled constraints: roleArn shape, stopConditions source/value, the two experimentOptions enums, and (for aws:fis:wait only) the duration parameter plus the startAfter graph. Several rules FIS enforces at template-create time are missing, so the emulator answers HTTP 200 and PERSISTS a template real FIS would have refused with ValidationException (400):

1. A target must identify its resources by EXACTLY ONE of `resourceArns` or `resourceTags`. FIS refuses a target that names both, and a target that names neither. The emulator stores whatever arrives (`state.rs::template_target` copies both members verbatim), so `{resourceType, selectionMode, resourceArns:[...], resourceTags:{...}}` and `{resourceType, selectionMode}` with neither member both succeed.

2. `actions.<name>.targets` must name targets the SAME template declares (and the map key must be a target type the action supports). `state.rs::actions_refusal` checks `startAfter` references but never the `targets` references, so an action pointing at a target name the template does not declare — or at a target of the wrong resource type for that action — is accepted and later started.

3. `selectionMode` is modelled as a free string but FIS admits only `ALL`, `COUNT(n)` and `PERCENT(n)`. Nothing refuses another value: `state.rs::selection_count` explicitly reads an unrecognised mode as `ALL` ("a value in none of the three documented forms is read as the documented default"), so `selectionMode: "SOME"` is stored and silently behaves as ALL instead of being refused.

4. `actions.<name>.actionId` is never checked against the FIS action catalog, even though the crate carries one (`state.rs::ACTION_CATALOG` / `action_catalog_entry`, which `GetAction` reports from). An actionId that names no FIS action (e.g. `aws:bogus:nonsense`) is stored and can be started.

5. Required action parameters are only enforced for `aws:fis:wait`. The catalog records `ActionCatalogParameter.required` for every entry (e.g. `duration` for `aws:ebs:pause-volume-io`, `documentArn` for `aws:ssm:send-command`, `instanceTerminationPercentage` for `aws:eks:terminate-nodegroup-instances`), yet `actions_refusal` returns early for any actionId other than the wait action, so a template whose action omits a required parameter is accepted.

Expected: each of these requests is refused with ValidationException (HTTP 400) and nothing is persisted. Actual: HTTP 200 with the template stored and readable through GetExperimentTemplate.

## Evidence

Smithy model: CreateExperimentTemplate declares ValidationException (HTTP 400); `targets` -> CreateExperimentTemplateTargetInputMap of CreateExperimentTemplateTargetInput (resourceType + selectionMode required, resourceArns/resourceTags both optional — the exclusivity is therefore a server-side rule); `actions` -> CreateExperimentTemplateActionInput with `targets` map and `actionId`. Well-established FIS behavior documented in the FIS User Guide: a target specifies resource IDs/ARNs OR resource tags and filters, not both; an action's target must reference a target defined in the same template; selectionMode is one of ALL | COUNT(n) | PERCENT(n); the action id must be one of FIS's published actions and its required parameters must be supplied.
Handler: services/.../src/ops/create_experiment_template.rs lines 174-204 (only role_arn_refusal, stop_conditions_refusal, template_experiment_options_refusal, actions_quota_refusal, actions_refusal are consulted); src/state.rs `actions_refusal` (checks startAfter + aws:fis:wait duration only, `if action.action_id != WAIT_ACTION_ID { continue; }`), `template_target` (copies resourceArns and resourceTags verbatim), `selection_count` (unknown mode read as ALL).
Tests: tests/all/control_plane_rules.rs asserts the wait-duration bound, the startAfter graph and "another action's parameters are stored as sent" — no test asserts any of the five rules above, and tests/all/list_experiment_resolved_targets.rs builds targets with both resourceTags and a selectionMode without exercising a refusal.

## Steps to Reproduce

1. Boot the emulator and point a botocore `fis` client at it.
2. Call create_experiment_template with targets={'web': {'resourceType': 'aws:ec2:instance', 'selectionMode': 'ALL', 'resourceArns': ['arn:aws:ec2:us-east-1:123456789012:instance/i-1'], 'resourceTags': {'Stage': 'test'}}} — observe HTTP 200 instead of ValidationException for naming both resourceArns and resourceTags.
3. Call create_experiment_template with a target that names NEITHER resourceArns nor resourceTags — observe HTTP 200.
4. Call create_experiment_template with actions={'stop': {'actionId': 'aws:ec2:stop-instances', 'targets': {'Instances': 'no-such-target'}}} and targets={} — observe HTTP 200 instead of a ValidationException naming the undefined target.
5. Call create_experiment_template with a target whose selectionMode is 'SOME' — observe HTTP 200, then start the experiment and see every resource resolved as if ALL had been requested.
6. Call create_experiment_template with actions={'x': {'actionId': 'aws:bogus:nonsense'}} — observe HTTP 200 and GetExperimentTemplate reporting the bogus action id.
7. Call create_experiment_template with actions={'pause': {'actionId': 'aws:ebs:pause-volume-io'}} (required `duration` omitted) — observe HTTP 200.
