Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6b193e67e38a36506ad31c413899507363d5dc88

# A put that drops a static-membership configuration strands the group's members: no call can ever remove them again

_Filed: 2026-10-28T05:58:34Z_
_Operation: PutGroupConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6b193e67e38a36506ad31c413899507363d5dc88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutGroupConfiguration` replaces a group's whole service configuration and never touches the static memberships the group holds, so a group that was populated through `GroupResources` can be left holding members while its *current* configuration declares none of the static-membership types. `UngroupResources` then refuses every removal for that group, because it gates on the configuration IN EFFECT (`static_membership_configuration_type()`), not on whether the group holds static memberships. The memberships keep being reported by `ListGroupResources` but can never be dropped — the only way to dispose of them is to delete the whole group.

This contradicts the premise `UngroupResources` writes down for its own gate ("a group outside that catalog holds no static membership to remove"), which this crate's own test disproves: it asserts the memberships survive exactly such a put. It also contradicts the modelled condition the same comment quotes: the operation "works only with static groups that you populated using the GroupResources operation" — and this group IS a static group populated using `GroupResources`.

Either side can carry the fix (the put refusing to drop a static-membership configuration while members exist, or the ungroup gating on the memberships the record holds rather than on the configuration in effect); what is defective today is that the two paths together make stored state unremovable.

## Evidence

Smithy model (`smithy_query(query="doc",shape="UngroupResources")`) states the condition the refused group satisfies:
  "Removes the specified resources from the specified group. This operation works only with static groups that you populated using the GroupResources operation. It doesn't work with any resource groups that are automatically populated by tag-based or CloudFormation stack-based queries."

The put replaces the whole configuration and touches nothing else — services/service-com-amazonaws-resourcegroups-ardi/src/ops/put_group_configuration.rs:265-268:
```
        // Accepted, not applied: the configuration becomes the proposed one and falls
        // due a span later, so a caller reading straight after this call still sees the
        // update in flight.
        record.put_configuration(input.configuration, now);
```
and services/service-com-amazonaws-resourcegroups-ardi/src/state.rs:666-669:
```
    /// It REPLACES the whole configuration rather than merging into it, and a
    /// request that carries no configuration item leaves the group with none at
    /// all.
```

The ungroup gate, with the premise that this makes false — services/service-com-amazonaws-resourcegroups-ardi/src/ops/ungroup_resources.rs:228-239:
```
        // Documented: this operation "works only with static groups that you
        // populated using the GroupResources operation" and "doesn't work with any
        // resource groups that are automatically populated by tag-based or
        // CloudFormation stack-based queries" — the same groups `GroupResources`
        // serves, since a group outside that catalog holds no static membership to
        // remove.
        if group.static_membership_configuration_type().is_none() {
            return Err(UngroupResourcesErrorModeled::BadRequestException(BadRequestException {
                message: Some(crate::state::unsupported_group_message(Self::NAME)),
            })
            .into());
        }
```
and the gate reads the configuration in effect, not the memberships — services/service-com-amazonaws-resourcegroups-ardi/src/state.rs:886-892:
```
    pub fn static_membership_configuration_type(&self) -> Option<&str> {
        self.effective_configuration()
            .into_iter()
            .flatten()
            .map(|item| item.r#type.as_str())
            .find(|declared| STATIC_MEMBERSHIP_CONFIGURATION_TYPES.contains(declared))
    }
```

The crate's own test proves the premise false (memberships survive a put that drops the static-membership type) — services/service-com-amazonaws-resourcegroups-ardi/tests/all/put_group_configuration.rs:746-766:
```
    // And a put that replaces that configuration takes the protection away with it,
    // while the memberships the group already holds stay where they are.
    let case = "a configuration that neither protects nor serves static membership";
    ... accepted(... "Configuration": [{ "Type": "AWS::ResourceGroups::Generic" }] ...)
    make_update_due(&emulator, name);
    let listed = emulator.ok("/list-group-resources", &json!({ "Group": name })).await;
    assert_eq!(
        listed["ResourceIdentifiers"][0]["ResourceArn"].as_str(),
        Some(member),
        "case {case:?}: expected the recorded membership to survive a put, got {listed}",
    );
```
That test stops at re-checking `GroupResources` (put_group_configuration.rs:767-772) and never calls `/ungroup-resources` in that state, so nothing pins the stranding.

And `UngroupResources` is the ONLY path that drops a membership — services/service-com-amazonaws-resourcegroups-ardi/src/state.rs:609-612:
```
    /// Static members added by `GroupResources` and dropped by
    /// `UngroupResources`, kept sorted by ARN: the sort is the listing order
    /// `ListGroupResources` pages through.
    pub members: Vec<MemberRecord>,
```

## Steps to Reproduce

1. CreateGroup with {"Name":"stranded","Configuration":[{"Type":"AWS::EC2::HostManagement"}]} so the group serves static membership.
2. GroupResources with {"Group":"stranded","ResourceArns":["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"]} — the ARN comes back in Succeeded.
3. PutGroupConfiguration with {"Group":"stranded","Configuration":[{"Type":"AWS::ResourceGroups::Generic"}]} and let the 500 ms CONFIGURATION_UPDATE_SETTLE_DELAY elapse (or move due_at as tests/all/put_group_configuration.rs's make_update_due does).
4. ListGroupResources with {"Group":"stranded"} — the instance ARN is still a member (pinned by tests/all/put_group_configuration.rs:759-766).
5. UngroupResources with {"Group":"stranded","ResourceArns":["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"]} — expected: the removal succeeds, because this is a static group populated using GroupResources. Actual: 400 BadRequestException "UngroupResources can only be used with a group whose configuration is one of AWS::EC2::CapacityReservationPool, AWS::EC2::HostManagement, AWS::ResourceGroups::ApplicationGroup; ...".
6. Repeat step 5 after any further put: the membership can never be removed; only DeleteGroup disposes of it.
