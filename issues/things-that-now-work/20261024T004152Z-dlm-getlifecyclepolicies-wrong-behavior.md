Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a1b03c4f6e81626c353b3ff836c908b12c596c5c

# A custom snapshot policy targeting INSTANCE is reported as PolicyType IMAGE_MANAGEMENT

_Filed: 2026-10-24T00:41:52Z_
_Operation: GetLifecyclePolicies | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a1b03c4f6e81626c353b3ff836c908b12c596c5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A CUSTOM policy whose `PolicyDetails` omits `PolicyType` must take the documented default `EBS_SNAPSHOT_MANAGEMENT`, whatever it targets: `PolicyDetails$ResourceTypes` documents `INSTANCE` as "create multi-volume snapshots from the volumes for an instance", i.e. a snapshot-management policy. The emulator instead derives the type from the targeted resource type and reports `IMAGE_MANAGEMENT` for any policy that targets `INSTANCE`.

Expected: `GetLifecyclePolicies` returns `Policies[*].PolicyType = "EBS_SNAPSHOT_MANAGEMENT"` for a custom policy created with `PolicyDetails = {ResourceTypes: ["INSTANCE"], TargetTags: [...], Schedules: [...]}` and no `PolicyType` (the documented default). Only a policy that explicitly states `PolicyType: IMAGE_MANAGEMENT`, or a DEFAULT policy created with `DefaultPolicy: INSTANCE` (which "creates EBS-backed AMIs"), is an `IMAGE_MANAGEMENT` policy.

Actual: the summary reports `IMAGE_MANAGEMENT`, so a client listing policies sees a multi-volume snapshot policy classified as an AMI policy, and cannot distinguish it from a real `IMAGE_MANAGEMENT` policy. The same helper also leaves `GetLifecyclePolicy`'s `Policy.PolicyDetails.PolicyType` unstamped for such a policy, so neither read reports the documented default correctly.

## Evidence

Spec: `com.amazonaws.dlm#PolicyDetails$PolicyType` — "The default is EBS_SNAPSHOT_MANAGEMENT"; `PolicyDetails$ResourceTypes` — "[Custom snapshot policies only] ... use INSTANCE to create multi-volume snapshots from the volumes for an instance"; `LifecyclePolicySummary$PolicyType` is the member returned by `GetLifecyclePolicies` (GET /policies).
Handler/state: `src/ops/get_lifecycle_policies.rs` builds every element with `crate::state::LifecyclePolicyRecord::to_summary`, which sets `policy_type: Some(self.policy_type())`; `src/state.rs` `policy_type()` returns `IMAGE_POLICY_TYPE` (`IMAGE_MANAGEMENT`) whenever `target_resource_type()` is `Some("INSTANCE")` — and `target_resource_type()` reads `PolicyDetails$ResourceTypes[0]` for a custom policy. The same module contradicts itself: `materialized_policy_details` computes `creates_snapshots = details.policy_type != Some(IMAGE_MANAGEMENT)`, so for this very policy it stamps `CreateRule.Location = CLOUD` (a `[Custom snapshot policies only]` member) while `policy_type()` calls it an AMI policy.
Tests: `tests/all/lifecycle_policy_crud.rs` only exercises custom policies that state `PolicyType` explicitly (`custom_snapshot_details()` = EBS_SNAPSHOT_MANAGEMENT + ResourceTypes [VOLUME], `custom_image_details()` = IMAGE_MANAGEMENT + ResourceTypes [INSTANCE]); the `PolicyType` assertion at lines 450-461 runs only for DEFAULT policies. No test covers a custom policy with `ResourceTypes: ["INSTANCE"]` and no `PolicyType`.

## Steps to Reproduce

1. POST /policies with {"ExecutionRoleArn": <emulator-account role ARN>, "Description": "multi volume snapshots", "State": "ENABLED", "PolicyDetails": {"ResourceTypes": ["INSTANCE"], "TargetTags": [{"Key": "backup", "Value": "daily"}], "Schedules": [{"Name": "daily", "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"}, "RetainRule": {"Count": 7}}]}} — note no PolicyType member
2. GET /policies and read Policies[0].PolicyType
3. Observe "IMAGE_MANAGEMENT" where the documented default for an omitted PolicyType is "EBS_SNAPSHOT_MANAGEMENT"
4. GET /policies/{PolicyId} and observe Policy.PolicyDetails.PolicyType is not reported at all for the same policy
