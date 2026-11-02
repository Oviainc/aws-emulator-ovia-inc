Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: f30b839d650f592709ba7d30fd0311226ca1f682

# A default-policy create silently discards PolicyDetails.PolicyType, leaving the stored configuration holding a type no read of it reports

_Filed: 2026-10-29T16:27:41Z_
_Operation: CreateLifecyclePolicy | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `f30b839d650f592709ba7d30fd0311226ca1f682`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

On the DEFAULT-policy create path the submitted `PolicyDetails$PolicyType` has NO effect on anything a caller can observe, and the stale value stays in the stored record.

`CreateLifecyclePolicy::handle` decides the kind (`is_default`) and hands the structure to `crate::state::materialized_policy_details(details, is_default)`. For a default policy that function returns on its second line after stamping `PolicyLanguage` only, so `PolicyType` is neither stamped nor cleared: whatever the request stated is persisted inside `policy_details`. Every read then ignores it — `LifecyclePolicyRecord::policy_type()` derives a default policy's type from `default_policy_type` alone, and `effective_policy_details()` overwrites `details.policy_type` with that derived value on every projection — so `GetLifecyclePolicy` and `GetLifecyclePolicies` report a type the request did not ask for, with no error.

Two defects follow from that one gap:

1. The member is silently dropped where the model says it decides what the policy manages. A create naming `DefaultPolicy: "VOLUME"` beside `PolicyDetails: {"PolicyLanguage": "SIMPLIFIED", "PolicyType": "IMAGE_MANAGEMENT"}` answers 200 and every read of the policy reports `PolicyType: "EBS_SNAPSHOT_MANAGEMENT"`. The caller asked, in the member whose documentation is "Specify IMAGE_MANAGEMENT to create a lifecycle policy that manages the lifecycle of EBS-backed AMIs", for an AMI policy and silently got a snapshot policy.

2. The record is left holding a configuration whose `PolicyType` contradicts the type every read of it reports — exactly the failure mode the sibling member's stamping exists to prevent. The crate states that invariant twice in the code under discussion (`materialized_policy_details`' doc: "A DEFAULT policy's configuration carries none of the custom members below"; `create_lifecycle_policy.rs`: "what is fixed here is that no record is left holding a configuration whose language contradicts its kind") and `UpdateLifecyclePolicy` enforces it: `immutable_configuration_violation` refuses a stated `PolicyType` that differs from `policy.policy_type()`. The identical structure the create accepted is therefore refused with 400 by the update, so one request body means two different things to the two write paths.

This is NOT the already-filed create gaps: `20261023T232846Z-dlm-createlifecyclepolicy-parameter-ignored` is about the request-BODY `[Default policies only]` members being dropped when `DefaultPolicy` is unnamed (they are now moved onto the record), and `...validation--r7` / `...wrong-behavior--r3` are about the RESOURCE type (`ResourceType` absent, `ResourceTypes` accumulating). Neither touches `PolicyDetails$PolicyType` on a default policy, which the crate normalizes for no kind of policy and reports from nowhere.

## Evidence

The model sentence the dropped member carries — `smithy_query(query="doc", shape="PolicyDetails", member="PolicyType")`:

> "The type of policy. Specify `EBS_SNAPSHOT_MANAGEMENT` to create a lifecycle policy that manages the lifecycle of Amazon EBS snapshots. Specify `IMAGE_MANAGEMENT` to create a lifecycle policy that manages the lifecycle of EBS-backed AMIs. Specify `EVENT_BASED_POLICY` to create an event-based policy ... The default is `EBS_SNAPSHOT_MANAGEMENT`."

The write path returns before the type is ever normalized for a default policy — `services/service-com-amazonaws-dlm-dlm-20180112/src/state.rs:383-393`:
```
pub fn materialized_policy_details(
    mut details: PolicyDetails,
    default_policy: bool,
) -> PolicyDetails {
    if default_policy {
        // A DEFAULT policy's configuration, which carries none of the custom
        // members stamped below.
        details.policy_language = Some(SIMPLIFIED_POLICY_LANGUAGE.to_string());
        return details;
    }
    details.policy_language = Some(STANDARD_POLICY_LANGUAGE.to_string());
```
while the same function's own doc asserts the opposite of what it stores — `src/state.rs:352-356`: "A DEFAULT policy's configuration carries none of the custom members below" — and `src/state.rs:365-367` lists `PolicyType` among the members it stamps: "* `PolicyType` — the documented default `EBS_SNAPSHOT_MANAGEMENT` for a configuration that names no type ... whatever the policy targets."

The create calls it and persists the result unchanged — `src/ops/create_lifecycle_policy.rs:382-384`:
```
        let mut policy_details = input
            .policy_details
            .map(|details| crate::state::materialized_policy_details(details, is_default));
```
under the comment that states the invariant this leaves broken for the type — `src/ops/create_lifecycle_policy.rs:364-381`: "The configuration is stored with every member the service materializes already stamped onto it, so a read reports the stored record rather than synthesizing a default per call. ... what is fixed here is that no record is left holding a configuration whose language contradicts its kind."

Every read discards the stored statement. `src/state.rs:686-697`:
```
    pub fn policy_type(&self) -> String {
        if self.default_policy {
            return match self.default_policy_type.as_deref() {
                Some(INSTANCE_DEFAULT_POLICY_TYPE) => IMAGE_POLICY_TYPE.to_string(),
                _ => DEFAULT_POLICY_TYPE.to_string(),
            };
        }
```
with its doc admitting the structure is not read (`src/state.rs:683-685`): "That is judged FIRST, so the type a default policy reports follows the kind of default policy it is however its configuration structure was written", and `src/state.rs:590-592` patching the projection instead of the record:
```
        let mut details = self.policy_details.clone().unwrap_or_default();
        details.policy_language = Some(self.policy_language().to_string());
        details.policy_type = Some(self.policy_type());
```
(`to_policy` -> `effective_policy_details`, `src/state.rs:784`; `to_summary` -> `policy_type()`, `src/state.rs:802`).

The sibling write path refuses the very structure the create accepted — `src/state.rs:1354-1364`, reached from `src/ops/update_lifecycle_policy.rs:281-287`:
```
    if let Some(stated) = details.policy_type.as_deref() {
        let stored = policy.policy_type();
        if stated != stored {
            return Some(configuration_member_cannot_be_updated(
                POLICY_TYPE_MEMBER,
                "policy type",
                stated,
                &stored,
            ));
        }
    }
```

Test coverage: none. `tests/all/create_policy_kind_materialization.rs:287-319` enumerates "The five ways to ask for this one kind of default policy" and no row states `PolicyType` inside a default policy's structure (`PolicyType` appears only in `custom_image_details()` at line 243 and `event_based_details()` at line 251, both CUSTOM policies), and `tests/all/configuration_kind_from_policy_type.rs` judges the type only on custom configurations — so the default-policy `PolicyType` path is unpinned on every read.

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with {"ExecutionRoleArn": "arn:aws:iam::<emulator account>:role/AWSDataLifecycleManagerDefaultRole", "Description": "ami default policy", "State": "ENABLED", "DefaultPolicy": "VOLUME", "PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "PolicyType": "IMAGE_MANAGEMENT"}} — observe HTTP 200 and note the PolicyId: the create accepted a configuration stating PolicyType IMAGE_MANAGEMENT
2. GET /policies/{PolicyId} — Policy.PolicyDetails reports "PolicyType": "EBS_SNAPSHOT_MANAGEMENT", not the IMAGE_MANAGEMENT the request stated: the member documented as "Specify IMAGE_MANAGEMENT to create a lifecycle policy that manages the lifecycle of EBS-backed AMIs" was silently discarded (src/state.rs:592 overwrites it from src/state.rs:686-697)
3. GET /policies — the summary for the same policy also reports "PolicyType": "EBS_SNAPSHOT_MANAGEMENT" (src/state.rs:802), so no read anywhere reports the stated type
4. PATCH /policies/{PolicyId} with {"PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "PolicyType": "IMAGE_MANAGEMENT"}} — i.e. resend the exact structure step 1 was accepted with — and observe HTTP 400 InvalidRequestException "The parameter PolicyDetails.PolicyType cannot be updated. The policy's policy type is EBS_SNAPSHOT_MANAGEMENT and IMAGE_MANAGEMENT was requested." (src/state.rs:1354-1364): the update refuses the configuration the create stored
5. Repeat step 1 with "DefaultPolicy": "INSTANCE" and "PolicyType": "EBS_SNAPSHOT_MANAGEMENT" — the reads report IMAGE_MANAGEMENT, the mirror image of the same silent discard
