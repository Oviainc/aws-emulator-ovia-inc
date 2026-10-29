Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: c550c8d329f766442e2f60b5ca19de7c84eb66f9

# A replacement PolicyDetails that adds an EventSource but omits PolicyType is accepted, so a custom snapshot policy silently becomes event-based and loses its ResourceTypes, TargetTags and Schedules

_Filed: 2026-10-28T01:42:21Z_
_Operation: UpdateLifecyclePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `c550c8d329f766442e2f60b5ca19de7c84eb66f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateLifecyclePolicy enforces two rules on a replacement PolicyDetails: the model's "You cannot update the policy type or the resource type" (immutable_configuration_violation), and "the replacement must be as complete as the one a create would have had to state" (omitted_configuration_members). An EventSource added to a CUSTOM SNAPSHOT policy's replacement configuration defeats both at once, because each of them reads a different member to decide what kind of policy the configuration describes.

* immutable_configuration_violation compares only the PolicyType MEMBER the request states (state.rs:1219). A replacement that states no PolicyType passes it, and carry_over_policy_type then stamps the policy's stored type (EBS_SNAPSHOT_MANAGEMENT) onto it.
* omitted_configuration_members asks configures_event_based_policy, which answers true on details.event_source.is_some() ALONE (state.rs:286). So the completeness rule stops requiring ResourceTypes, TargetTags and Schedules and requires only EventSource and Actions - both of which the request carries.

The update is therefore accepted (HTTP 200) and policy.policy_details is replaced wholesale with {PolicyType: EBS_SNAPSHOT_MANAGEMENT, PolicyLanguage: STANDARD, EventSource, Actions}: the policy's schedules, target tags and target resource type are DESTROYED, target_resource_types() becomes empty so GET /policies?resourceTypes=VOLUME stops selecting it, and both reads report a snapshot-typed policy holding a member the model documents [Event-based policies only] and no schedule at all. The very same request WITH "PolicyType": "EVENT_BASED_POLICY" spelled out is refused 400 - so the kind change the model rules out is refused when stated honestly and performed when the member is simply left out. An update with PolicyDetails: {} is correctly refused for the three omitted members; the EventSource is what flips the required set.

The same helper pair is called by CreateLifecyclePolicy (create_lifecycle_policy.rs:298, :388), where a create of PolicyDetails: {EventSource, Actions} with no PolicyType is likewise accepted and stored as EBS_SNAPSHOT_MANAGEMENT - directly contradicting the crate's own comment that "there is no accepted event-based policy whose type was left unstated". The fix belongs in the shared state helpers, so both write paths are healed by one change.

## Evidence

The model rule the update is meant to enforce - smithy_query(query="doc", shape="UpdateLifecyclePolicyRequest", member="PolicyDetails"): "The configuration of the lifecycle policy. You cannot update the policy type or the resource type." And the member that makes a configuration event-based - smithy_query(query="doc", shape="PolicyDetails", member="EventSource"): "<b>[Event-based policies only]</b>  The event that activates the event-based policy."

The crate's OWN definition of what makes a configuration event-based - src/state.rs:277-287:
```
/// True when `details` configures an EVENT-BASED policy rather than a custom
/// snapshot or AMI one.
///
/// `PolicyDetails$PolicyType` names the type, and `EventSource` ("[Event-based
/// policies only] The event that activates the event-based policy") is declared
/// by that kind of policy alone, so either of them makes the configuration an
/// event-based one.
pub fn configures_event_based_policy(details: &PolicyDetails) -> bool {
    details.policy_type.as_deref() == Some(EVENT_BASED_POLICY_TYPE)
        || details.event_source.is_some()
}
```

The immutability check that nevertheless reads only the stated PolicyType member - src/state.rs:1219-1229:
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
```
reached from src/ops/update_lifecycle_policy.rs:279-285, and followed by src/ops/update_lifecycle_policy.rs:292-294:
```
        if let Some(details) = input.policy_details.as_mut() {
            crate::state::carry_over_policy_type(policy, details);
        }
```

The completeness rule that then asks for the WRONG member list - src/state.rs:1509-1521:
```
pub fn omitted_configuration_members(details: &PolicyDetails) -> Vec<String> {
    let required: Vec<(&str, bool)> = if configures_event_based_policy(details) {
        vec![
            ("EventSource", details.event_source.is_some()),
            ("Actions", details.actions.is_some()),
        ]
    } else {
        vec![
            ("ResourceTypes", details.resource_types.is_some()),
            ("TargetTags", details.target_tags.is_some()),
            ("Schedules", details.schedules.is_some()),
        ]
    };
```
called under the comment that states the rule being broken - src/ops/update_lifecycle_policy.rs:296-303: "A `PolicyDetails` on an update REPLACES the stored configuration wholesale, so the replacement must be as complete as the one a create would have had to state: the members the kind of policy it describes is built from."

The crate ASSERTS the state this produces cannot exist - src/state.rs:352-362:
```
    // `PolicyType` - the documented default for a configuration that names no
    // type: "The default is `EBS_SNAPSHOT_MANAGEMENT`." ... That holds for a configuration
    // carrying an `EventSource` and no type too: by the same documentation such
    // a request describes a snapshot policy that omits the members one is built
    // from, so there is no accepted event-based policy whose type was left
    // unstated, and guessing the type from which members a configuration
    // happens to carry is the very inference this default replaces.
```
yet the lines after it stamp that type and return before any custom member is materialized - src/state.rs:368-377:
```
    details
        .policy_type
        .get_or_insert_with(|| DEFAULT_POLICY_TYPE.to_string());
...
    if event_based {
        return details;
    }
```

Test coverage: every EventSource row under tests/ states the type explicitly, so the type-unstated case is unpinned - tests/all/policy_configuration_validation.rs:319-322 (fn event_based_details() begins `"PolicyType": "EVENT_BASED_POLICY", "EventSource": {`), tests/all/policy_type.rs:216-219, tests/all/policy_identity_immutability.rs:243-248. tests/all/policy_configuration_validation.rs:500-504 pins only ("an event-based policy", event_based_details(), &["EventSource", "Actions"]), i.e. a configuration that already names the event-based type.

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with {"ExecutionRoleArn": "arn:aws:iam::<emulator account>:role/AWSDataLifecycleManagerDefaultRole", "Description": "custom snapshot policy", "State": "ENABLED", "PolicyDetails": {"ResourceTypes": ["VOLUME"], "TargetTags": [{"Key": "backup", "Value": "daily"}], "Schedules": [{"Name": "daily", "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"}, "RetainRule": {"Count": 7}}]}} - observe HTTP 200 and note the PolicyId
2. GET /policies/{PolicyId} - Policy.PolicyDetails reports PolicyType EBS_SNAPSHOT_MANAGEMENT with its ResourceTypes, TargetTags and Schedules
3. PATCH /policies/{PolicyId} with {"PolicyDetails": {"PolicyType": "EVENT_BASED_POLICY", "EventSource": {"Type": "MANAGED_CWE", "Parameters": {"EventType": "shareSnapshot", "SnapshotOwner": ["000000000000"], "DescriptionRegex": "^.*$"}}, "Actions": [{"Name": "copy", "CrossRegionCopy": [{"Target": "us-west-2", "EncryptionConfiguration": {"Encrypted": false}, "RetainRule": {"Interval": 1, "IntervalUnit": "DAYS"}}]}]}} - observe HTTP 400 InvalidRequestException "The parameter PolicyDetails.PolicyType cannot be updated...": the honest kind change is correctly refused
4. PATCH /policies/{PolicyId} again with the SAME structure minus the "PolicyType" member - observe HTTP 200
5. GET /policies/{PolicyId} - Policy.PolicyDetails now reports {"PolicyType": "EBS_SNAPSHOT_MANAGEMENT", "PolicyLanguage": "STANDARD", "EventSource": {...}, "Actions": [...]}: ResourceTypes, TargetTags and Schedules are gone from the stored configuration, and a snapshot-typed policy now holds an [Event-based policies only] member and no schedule
6. GET /policies?resourceTypes=VOLUME - the policy is no longer selected, because target_resource_types() is now empty; the configuration it was created with cannot be recovered
7. PATCH /policies/{PolicyId} with {"PolicyDetails": {}} - observe HTTP 400 naming PolicyDetails.ResourceTypes, PolicyDetails.TargetTags and PolicyDetails.Schedules, showing that it is the EventSource alone that switched off the completeness rule
8. Create path, same shared helper: POST /policies with {"ExecutionRoleArn": ..., "Description": "event based no type", "State": "ENABLED", "PolicyDetails": {"EventSource": {...}, "Actions": [...]}} and no PolicyType - observe HTTP 200, then GET /policies/{PolicyId} reports PolicyType EBS_SNAPSHOT_MANAGEMENT for a configuration with an EventSource and no schedules, contradicting src/state.rs:357-361
