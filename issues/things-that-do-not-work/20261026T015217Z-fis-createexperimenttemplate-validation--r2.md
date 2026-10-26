Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8

# A template's targets are never judged against the published target-resource-type catalog, while its actions are judged against the action catalog

_Filed: 2026-10-26T01:52:17Z_
_Operation: CreateExperimentTemplate | Class: validation_

> _Note: this issue was identified against the code base at commit `6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::template_refusal` — the one sequence both `CreateExperimentTemplate` and `UpdateExperimentTemplate` judge a template by — applies the published-catalog contract to the template's ACTIONS but not to its TARGETS.

For an action, `actions_refusal` refuses an `actionId` the crate's own `ACTION_CATALOG` holds no entry for, and refuses an action that omits a parameter the catalogued entry marks `required`.

For a target, `targets_refusal` applies only two rules (resourceArns/resourceTags exclusivity and the `selectionMode` form). It never consults `target_resource_type_catalog_entry` / `TARGET_RESOURCE_TYPE_CATALOG`, even though:

* the catalog is the same service-owned table `GetTargetResourceType` reports from, so a `resourceType` it holds no entry for names a type this service answers `ResourceNotFoundException` for — yet a template declaring such a target is accepted and persisted; and
* each catalogued row records `required` per parameter, with the crate's own comment saying that flag means "whether a target of this resource type HAS TO set the parameter" — yet a target of `aws:ecs:task` (whose `cluster` and `service` are both `required: true`) or of `aws:eks:pod` (`clusterIdentifier`, `namespace`, `selectorType` all `required: true`) with an empty or absent `parameters` map is accepted and persisted.

So one half of the crate's own catalog contract is enforced and the identical other half is not: the template that reaches the store describes a target resolution the service's own published contract says cannot be identified. (This is a DIFFERENT rule from the already-fixed CreateExperimentTemplate validation gaps, which were target arns/tags exclusivity, the action->target reference, the `selectionMode` form, unknown `actionId` and missing required ACTION parameters.)

## Evidence

The action half, enforced — services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:3363-3368:
```
        let action_id = action.action_id.as_deref().unwrap_or_default();
        let Some(entry) = action_catalog_entry(action_id) else {
            return Some(invalid_input(format!(
                "actions.{name}.actionId must be an action FIS publishes, got {action_id:?}"
            )));
        };
```
and its stated rationale — state.rs:3383-3387:
```
/// `ActionParameter$required` is reported by `GetAction` for every published action, so
/// which parameters an action needs is part of the contract this service already serves —
/// the same table decides it here, rather than only the wait action being checked.
```

The target half, NOT enforced — state.rs:3259-3271 (the complete list of rules `targets_refusal` applies) and its body state.rs:3279-3307, which checks only `names_arns && names_tags`, `!names_arns && !names_tags` and `selection_mode_count(selection_mode, 0).is_none()`. `target.resource_type` is read nowhere in that function; the only reads of the resource-type catalog in the crate are state.rs:2489 (page token) and ops/get_target_resource_type.rs:149.

The rule the catalog's own comment states — state.rs:5899-5901:
```
    /// `TargetResourceTypeParameter$required` — whether a target of this resource type has
    /// to set the parameter.
    pub required: bool,
```
and the rows that carry it — state.rs:6031-6044:
```
        id: "aws:ecs:task",
        description: "An Amazon ECS task.",
        parameters: &[
            TargetResourceTypeCatalogParameter { name: "cluster", ... required: true, },
            TargetResourceTypeCatalogParameter { name: "service", ... required: true, },
        ],
```
(state.rs:6056-6085 does the same for `aws:eks:pod`.)

Test coverage: tests/all/get_target_resource_type.rs:199-219 pins exactly those catalogued `required` flags as reported by `GetTargetResourceType` (`parameters: &[("cluster", true), ("service", true)]`), while tests/all/template_target_and_action_rules.rs:12-22 enumerates the rules a template's targets ARE judged by — only the arns/tags exclusivity and the `selectionMode` forms — so no test pins the current acceptance, and no comment in the crate justifies the omission.

## Steps to Reproduce

1. Boot the emulator and point a `fis` client at it.
2. CreateExperimentTemplate with actions={'wait': {'actionId': 'aws:fis:wait', 'parameters': {'duration': 'PT5M'}}}, a valid roleArn/stopConditions, and targets={'tasks': {'resourceType': 'aws:ecs:task', 'selectionMode': 'ALL', 'resourceArns': ['arn:aws:ecs:us-east-1:000000000000:task/t-1']}} — no `parameters` at all. Observe HTTP 200 and GetExperimentTemplate reporting the stored target, although the service's own GetTargetResourceType('aws:ecs:task') reports `cluster` and `service` as required: true.
3. Repeat with targets={'x': {'resourceType': 'aws:bogus:nonsense', 'selectionMode': 'ALL', 'resourceTags': {'Stage': 'test'}}} — observe HTTP 200, while GetTargetResourceType('aws:bogus:nonsense') answers ResourceNotFoundException (404).
4. Compare with actions={'x': {'actionId': 'aws:bogus:nonsense'}}, which IS refused with ValidationException by the same refusal sequence.
