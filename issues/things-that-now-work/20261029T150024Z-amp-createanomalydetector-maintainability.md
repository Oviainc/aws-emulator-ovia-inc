Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: ee0cc881641f9455cbcb1e70f91e773fdf19df64

# The workspace-not-found refusal is composed inline in 28 handlers, although two of them already single-home it and state.rs single-homes the sibling refusals

_Filed: 2026-10-29T15:00:24Z_
_Operation: CreateAnomalyDetector | Class: maintainability_

> _Note: this issue was identified against the code base at commit `ee0cc881641f9455cbcb1e70f91e773fdf19df64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation addressed by `/workspaces/{workspaceId}` refuses an unresolved parent with the identical three-member `ResourceNotFoundException` — `message: crate::state::WORKSPACE_NOT_FOUND_MESSAGE`, `resource_id: input.workspace_id`, `resource_type: crate::state::WORKSPACE_RESOURCE_TYPE` — and 26 of them build it by hand inside the handler body, at 28 separate construction sites. Two handlers already pull it into a private `fn workspace_not_found(...)`, which is the shape this wants, but each keeps its own copy. `crate::state` already owns the other cross-handler refusals for exactly this reason (`arn_not_found` for the three tagging operations, `stale_revision_rejection` for the two resource-policy operations), and `ResourceNotFoundException` is one shared type in `crate::ops::types`, so one `crate::state::workspace_not_found(workspace_id) -> ResourceNotFoundException` that each handler wraps in its own modelled-error variant would own the composition once. As it stands, a change to the composition (the `resourceType` name, or reporting the workspace ARN rather than the id) has to be made in 28 places and can silently be made in fewer.

The fix is a pure move: the values built are byte-identical, so every response and every existing test stays unchanged.

## Evidence

The same composition, by hand, in 26 handler bodies — for example src/ops/describe_workspace.rs:169-178:
```
        let Some(workspace) = store.workspaces.get(&input.workspace_id) else {
            return Err(DescribeWorkspaceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::WORKSPACE_NOT_FOUND_MESSAGE.to_string(),
                    resource_id: input.workspace_id.clone(),
                    resource_type: crate::state::WORKSPACE_RESOURCE_TYPE.to_string(),
                },
            )
            .into());
        };
```
and already factored out, twice, with the same body — src/ops/create_anomaly_detector.rs:396-403:
```
fn workspace_not_found(workspace_id: &str) -> CreateAnomalyDetectorError {
    CreateAnomalyDetectorErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: crate::state::WORKSPACE_NOT_FOUND_MESSAGE.to_string(),
        resource_id: workspace_id.to_string(),
        resource_type: crate::state::WORKSPACE_RESOURCE_TYPE.to_string(),
    })
    .into()
}
```
src/ops/put_anomaly_detector.rs:428-435 is the identical function under its own error type.

The 28 construction sites (`grep -n WORKSPACE_NOT_FOUND_MESSAGE src/ops/*.rs`): create_alert_manager_definition.rs:208, create_anomaly_detector.rs:398, create_logging_configuration.rs:188, create_query_logging_configuration.rs:185, create_rule_groups_namespace.rs:247, delete_alert_manager_definition.rs:200, delete_anomaly_detector.rs:202, delete_logging_configuration.rs:205, delete_query_logging_configuration.rs:199, delete_resource_policy.rs:199, delete_rule_groups_namespace.rs:206, delete_workspace.rs:200, describe_alert_manager_definition.rs:177, describe_anomaly_detector.rs:177, describe_logging_configuration.rs:175, describe_query_logging_configuration.rs:178, describe_resource_policy.rs:181, describe_rule_groups_namespace.rs:180, describe_workspace.rs:172, describe_workspace_configuration.rs:179, list_anomaly_detectors.rs:201, list_rule_groups_namespaces.rs:205, put_alert_manager_definition.rs:208, put_anomaly_detector.rs:430, put_resource_policy.rs:200 and :214, put_rule_groups_namespace.rs:221, update_logging_configuration.rs:189, update_query_logging_configuration.rs:183, update_workspace_alias.rs:204, update_workspace_configuration.rs:222.

The crate's own stated rule for a refusal more than one handler answers — state.rs:1679-1688:
```
/// The refusal the three ARN-addressed `/tags/{resourceArn}` operations share for a
/// `resourceArn` that resolves to no live resource ...
/// ONE composition for all three, so `TagResource`, `UntagResource` and
/// `ListTagsForResource` cannot drift apart, and generic over the operation's modelled
/// error enum so each returns it through its own error channel.
pub fn arn_not_found<E>() -> crate::scaffolded::Scaffolded<E> {
```
(`stale_revision_rejection`, state.rs:1706-1708, does the same for the two resource-policy handlers and returns the shared `ValidationException` for each to wrap.)

Tests that cover the behaviour and must stay unchanged: tests/all/workspace_lifecycle.rs, tests/all/region_scoping.rs, tests/all/rejection_codes.rs, tests/all/kms_key_arn_and_parent_resolution.rs, tests/all/logging_configuration.rs, tests/all/query_logging_configuration.rs, tests/all/rule_groups_namespace_lifecycle.rs, tests/all/alert_manager_definition.rs and tests/all/resource_policy_lifecycle.rs — all assert the message/resourceId/resourceType the composition produces today.

## Steps to Reproduce

1. Run `grep -n WORKSPACE_NOT_FOUND_MESSAGE services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/*.rs` and count 28 construction sites across 28 handlers
2. Read src/ops/create_anomaly_detector.rs:396-403 and src/ops/put_anomaly_detector.rs:428-435 side by side: the same private helper, twice
3. Compare with state.rs:1679-1688, where the analogous refusal for the three tagging operations is single-homed with the explicit rationale "ONE composition for all three, so ... cannot drift apart"
4. Change the `resource_type` the refusal reports in one handler and observe the other 27 keep the old value
