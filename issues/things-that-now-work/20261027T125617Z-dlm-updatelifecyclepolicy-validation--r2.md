Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: 24ee3fd8efff19f858526857f11b5601c902b977

# An update changes a custom policy's PolicyType and ResourceTypes, which the model says cannot be updated

_Filed: 2026-10-27T12:56:17Z_
_Operation: UpdateLifecyclePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `24ee3fd8efff19f858526857f11b5601c902b977`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateLifecyclePolicyRequest$PolicyDetails` is documented "The configuration of the lifecycle policy. You cannot update the policy type or the resource type." The crate quotes that sentence three times (the handler's own module doc, the handler comment, and the `state.rs` helper doc) and implements only ONE half of it: the DEFAULT policy's `PolicyDetails$ResourceType`, guarded by `crate::state::resource_type_cannot_be_updated`. The two members the sentence names for a CUSTOM policy are left unguarded:

1. `PolicyDetails$PolicyType` ("The type of policy ... The default is EBS_SNAPSHOT_MANAGEMENT") — the replacement configuration is stored wholesale, so an update stating `PolicyType: IMAGE_MANAGEMENT` on a policy created as `EBS_SNAPSHOT_MANAGEMENT` (or stating `EVENT_BASED_POLICY` on a snapshot policy) is accepted with HTTP 200 and `GetLifecyclePolicy` / `GetLifecyclePolicies` then report the new type, because `LifecyclePolicyRecord::policy_type()` for a custom policy reads the stored structure. Worse, a replacement that simply OMITS `PolicyType` has the documented default stamped onto it by `materialized_policy_details`, so an AMI policy is silently converted into an `EBS_SNAPSHOT_MANAGEMENT` policy — a type change the caller never asked for.
2. `PolicyDetails$ResourceTypes` ("The target resource type for snapshot and AMI lifecycle policies") — the resource type of a custom policy. An update may freely move it from `INSTANCE` to `VOLUME`; nothing in `src/ops/update_lifecycle_policy.rs` reads `details.resource_types` at all (grep over `src/` finds it only in `omitted_configuration_members` and the `resourceTypes` filter), so the stored value is replaced and the `GetLifecyclePolicies` `resourceTypes` filter starts selecting the policy under a different type.

So the one change the model explicitly rules out for this operation is the one change it performs silently, while the generic helper the crate already wrote for exactly this refusal (`configuration_member_cannot_be_updated`) is called for the resource type of a default policy and for `PolicyLanguage`, but never for the policy type or the custom resource type.

## Evidence

Model (kind A) — `smithy_query(query="doc", shape="UpdateLifecyclePolicyRequest", member="PolicyDetails")`:
> "The configuration of the lifecycle policy. You cannot update the policy type or the resource type."

and `smithy_query(query="doc", shape="PolicyDetails", member="PolicyType")`: "The type of policy. Specify EBS_SNAPSHOT_MANAGEMENT ... Specify IMAGE_MANAGEMENT ... The default is EBS_SNAPSHOT_MANAGEMENT."; `smithy_query(query="doc", shape="PolicyDetails", member="ResourceTypes")`: "[Custom snapshot policies only] The target resource type for snapshot and AMI lifecycle policies."

The crate repeats the sentence and then guards only the default policy's resource type — `src/ops/update_lifecycle_policy.rs:75-76`:
```
//! * `PolicyDetails` (PolicyDetails) — The configuration of the lifecycle policy. You cannot
//!   update the policy type or the resource type.
```
`src/ops/update_lifecycle_policy.rs:302-326`:
```
        // The one member of that configuration an update may not CHANGE:
        // `UpdateLifecyclePolicyRequest$PolicyDetails` is "the configuration of
        // the lifecycle policy. You cannot update the policy type or the resource
        // type". ...
        if let Some(stated) = input
            .policy_details
            .as_ref()
            .and_then(|details| details.resource_type.as_deref())
        {
            if let Some(stored) = policy.default_policy_type.as_deref() {
                if stored != stated {
                    return Err(... resource_type_cannot_be_updated(stated, stored) ...);
```
No sibling check exists for `details.policy_type` or `details.resource_types`: the replacement is stored as-is — `src/ops/update_lifecycle_policy.rs:372-399`:
```
        if let Some(policy_details) = input.policy_details {
            ...
            let mut details =
                crate::state::materialized_policy_details(policy_details, default_policy);
            ...
            policy.policy_details = Some(details);
```
and the omitted type is replaced by the stamped default — `src/state.rs:363-365`:
```
    details
        .policy_type
        .get_or_insert_with(|| DEFAULT_POLICY_TYPE.to_string());
```
which is exactly what both read projections then report — `src/state.rs:628-631`:
```
        self.policy_details
            .as_ref()
            .and_then(|details| details.policy_type.clone())
            .unwrap_or_else(|| DEFAULT_POLICY_TYPE.to_string())
```
The refusal helper is already generic over the member and is used for the two other identity members — `src/state.rs:1113-1136`:
```
/// `UpdateLifecyclePolicyRequest$PolicyDetails` is "the configuration of the
/// lifecycle policy. You cannot update the policy type or the resource type": the
/// request states a configuration the service cannot apply to this policy ...
pub fn configuration_member_cannot_be_updated(
...
pub fn resource_type_cannot_be_updated(stated: &str, stored: &str) -> InvalidRequestException {
    configuration_member_cannot_be_updated(RESOURCE_TYPE_MEMBER, "resource type", stated, stored)
}
```

Test coverage: the behaviour is currently pinned the other way by `tests/all/policy_type.rs:444-468`, whose stated reason is about REPORTING consistency, not about permission to change the type:
```
/// The rule holds on the UPDATE path too: a replacement configuration is reported
/// with the type IT states, or the documented default when it states none.
#[tokio::test]
async fn a_replaced_configuration_reports_the_type_it_states() {
...
            "an AMI policy replaced by a configuration stating no type",
            json!(custom_details(Some(IMAGE_POLICY_TYPE), "INSTANCE")),
            json!(custom_details(None, "INSTANCE")),
            SNAPSHOT_POLICY_TYPE,
```
The quoted model sentence contradicts that reason: rows 1 and 2 of that table are precisely a policy-type update, and row 3 (`custom_details(None, "INSTANCE")` replaced by `custom_details(None, "VOLUME")`) is precisely a resource-type update, all asserted to answer HTTP 200. No test pins a refusal (`grep "cannot be updated"` over `tests/` matches only `tests/all/default_policy_configuration_precedence.rs:400`, the DEFAULT policy's `ResourceType`).

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with {"ExecutionRoleArn": "arn:aws:iam::<emulator account>:role/AWSDataLifecycleManagerDefaultRole", "Description": "an AMI policy", "State": "ENABLED", "PolicyDetails": {"PolicyType": "IMAGE_MANAGEMENT", "ResourceTypes": ["INSTANCE"], "TargetTags": [{"Key": "k", "Value": "v"}], "Schedules": [{"Name": "s", "CreateRule": {"Interval": 12, "IntervalUnit": "HOURS"}, "RetainRule": {"Count": 7}}]}} — note the PolicyId
2. GET /policies/{PolicyId} — Policy.PolicyDetails.PolicyType reports IMAGE_MANAGEMENT and GET /policies reports Policies[0].PolicyType = IMAGE_MANAGEMENT
3. PATCH /policies/{PolicyId} with {"PolicyDetails": {"PolicyType": "EBS_SNAPSHOT_MANAGEMENT", "ResourceTypes": ["VOLUME"], "TargetTags": [{"Key": "k", "Value": "v"}], "Schedules": [{"Name": "s", "CreateRule": {"Interval": 12, "IntervalUnit": "HOURS"}, "RetainRule": {"Count": 7}}]}} — observe HTTP 200, although `UpdateLifecyclePolicyRequest$PolicyDetails` documents "You cannot update the policy type or the resource type"
4. GET /policies/{PolicyId} — Policy.PolicyDetails.PolicyType is now EBS_SNAPSHOT_MANAGEMENT and ResourceTypes is ["VOLUME"]: both documented-immutable members changed
5. GET /policies?resourceTypes=VOLUME — the policy is now selected by the VOLUME filter, and GET /policies?resourceTypes=INSTANCE no longer returns it
6. Repeat step 3 with the same PolicyDetails but with "PolicyType" OMITTED — observe HTTP 200 and a read reporting PolicyType EBS_SNAPSHOT_MANAGEMENT: `materialized_policy_details` stamped the documented default over the stored IMAGE_MANAGEMENT, silently converting the policy's type
