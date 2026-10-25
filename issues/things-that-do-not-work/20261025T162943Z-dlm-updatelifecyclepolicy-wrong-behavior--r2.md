Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a4b61295d097ae4da0c45de1d4f9e67ffc2c86fd

# A default policy's PolicyDetails.ResourceType is stored twice, so an update of it is unobservable in reads yet changes the resourceTypes filter

_Filed: 2026-10-25T16:29:43Z_
_Operation: UpdateLifecyclePolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a4b61295d097ae4da0c45de1d4f9e67ffc2c86fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PolicyDetails$ResourceType` is the one `[Default policies only]` member the crate does NOT resolve into a single stored value. `LifecyclePolicyRecord` keeps the create-time value in `default_policy_type` and, independently, whatever `PolicyDetails.ResourceType` the last accepted write stated inside `policy_details` (`DefaultPolicyConfiguration::taken_from_policy_details` takes the other six members but not `resource_type`).

On the update path this produces three contradictory answers for one policy:

1. `UpdateLifecyclePolicy` ACCEPTS `PolicyDetails.ResourceType` on a default policy — the `DefaultPolicyOnlyParameters::in_policy_details` refusal (which lists `ResourceType`) runs only inside `if !policy.default_policy` — and persists it with `policy.policy_details = Some(details)`, while never touching `policy.default_policy_type`.
2. Both read projections then IGNORE what was persisted: `effective_policy_details` resolves `details.resource_type = self.default_policy_type.or(details.resource_type)`, so the stale create-time type always wins, and `policy_type()` derives the reported `PolicyType` from that same stale value.
3. `target_resource_types()` reports BOTH copies, so `GET /policies?resourceTypes=<new type>` matches the policy for a type no read of it reports — and `?resourceTypes=<old type>` still matches a configuration that no longer states it.

The net effect: an update answered 200 writes a value into the record that only the list filter can see, exactly the "write to the losing place unobservable" state the crate's own state module says its write paths exist to prevent. The same double-store is reachable on the create path (`DefaultPolicy: VOLUME` beside `PolicyDetails: {PolicyLanguage: SIMPLIFIED, ResourceType: INSTANCE}` is not refused, since the create's duplication check reads body members only and `ResourceType` has no body counterpart).

It also makes the create and update paths disagree about identical stored configurations: a policy CREATED with `PolicyDetails {PolicyLanguage: SIMPLIFIED, ResourceType: INSTANCE}` reports `PolicyType IMAGE_MANAGEMENT`, while a VOLUME default policy UPDATED to the same configuration reports `EBS_SNAPSHOT_MANAGEMENT`.

## Evidence

Handler accepts and stores it — `services/service-com-amazonaws-dlm-dlm-20180112/src/ops/update_lifecycle_policy.rs:228-235`: "if !policy.default_policy { let mut refused = default_policy_only_body_parameters(&input).stated_names(\"\"); refused.extend(crate::state::DefaultPolicyOnlyParameters::in_policy_details(input.policy_details.as_ref()).stated_names(crate::state::POLICY_DETAILS_PREFIX));" (the only refusal that names `ResourceType`, skipped for a default policy), then lines 336-340: "let mut details = crate::state::materialized_policy_details(policy_details); ... policy.policy_details = Some(details);".

The handler's own doc says it cannot change — `src/ops/update_lifecycle_policy.rs:375-376`: "`PolicyDetails$ResourceType` has no request-body counterpart to read: the resource type is not something an update may change."

The rule the storage breaks — `src/state.rs:390-396` (`DefaultPolicyConfiguration`): "Every one of these members is declared twice over ... Two stored copies of one member would need a precedence between them, and any fixed precedence makes a write to the losing place unobservable, so the write paths resolve the two into this ONE value and the read reports it." `taken_from_policy_details` (`src/state.rs:414-423`) takes `create_interval`, `retain_interval`, `copy_tags`, `extend_deletion`, `cross_region_copy_targets`, `exclusions` — and not `resource_type`.

The read drops it — `src/state.rs:509-512`: "details.resource_type = self.default_policy_type.clone().or(details.resource_type.clone());" and `src/state.rs:562-566`: "if self.default_policy { return match self.default_policy_target().as_deref() { Some(INSTANCE_DEFAULT_POLICY_TYPE) => IMAGE_POLICY_TYPE.to_string(), _ => DEFAULT_POLICY_TYPE.to_string() }; }" with `default_policy_target` (`src/state.rs:578-585`) preferring `self.default_policy_type` over `details.resource_type`.

The filter sees it — `src/state.rs:588-601`: "if let Some(default_type) = self.default_policy_type.as_ref() { types.push(default_type.clone()); } if let Some(details) = ... { if let Some(resource_type) = details.resource_type.as_ref() { types.push(resource_type.clone()); }", consumed by `src/ops/get_lifecycle_policies.rs:205-210`: "let targeted = policy.target_resource_types(); wanted.iter().all(|want| targeted.contains(want))".

Create/update disagreement — `tests/all/policy_type.rs:311-323` pins that a create of `{"PolicyLanguage": "SIMPLIFIED", "ResourceType": "INSTANCE"}` reports `IMAGE_MANAGEMENT`.

Test coverage gap — `tests/all/default_policy_configuration_precedence.rs:397-402` asserts "the resource type cannot be updated", but `update_request` (lines 215-226) always restates the SAME `ResourceType: DEFAULT_POLICY_TYPE` (`"VOLUME"`, line 56), so an update that states a DIFFERENT resource type is unasserted.

## Steps to Reproduce

1. POST /policies (signed for any region) with {"ExecutionRoleArn": <emulator-account role ARN>, "Description": "default volume policy", "State": "ENABLED", "DefaultPolicy": "VOLUME"} and note the PolicyId
2. PATCH /policies/{PolicyId} with {"PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "ResourceType": "INSTANCE"}} — observe HTTP 200 (the value is persisted into policy_details)
3. GET /policies/{PolicyId} — Policy.PolicyDetails.ResourceType still reports "VOLUME" and the summary type stays EBS_SNAPSHOT_MANAGEMENT, so the accepted update is invisible
4. GET /policies?resourceTypes=INSTANCE — the same policy IS returned, because target_resource_types() reports the value the read refuses to report; GET /policies?resourceTypes=VOLUME returns it too
5. Compare with a fresh create of {"PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "ResourceType": "INSTANCE"}}, which reports PolicyType IMAGE_MANAGEMENT for the identical stored configuration
