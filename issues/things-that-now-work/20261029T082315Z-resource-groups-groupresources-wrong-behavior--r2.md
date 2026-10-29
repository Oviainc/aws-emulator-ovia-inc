Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 696824a9cff68a02bfaecbfcac1fbeced7aa5eee

# GroupResources answers an empty `Succeeded` list, a value the modelled `ResourceArnList` does not admit

_Filed: 2026-10-29T08:23:15Z_
_Operation: GroupResources | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `696824a9cff68a02bfaecbfcac1fbeced7aa5eee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When every ARN a request names is already a member of the group — the documented idempotent repeat of a grouping — the handler answers HTTP 200 with `"Succeeded": []`. The model targets that output member at `ResourceArnList`, whose `@length` is `min: 1, max: 10`, so an empty list is not a value `Succeeded` can carry; the "nothing was added" case has to be expressed by the member being absent, not by emitting a list the constraint excludes. The two sibling members of the same response are modelled WITHOUT a minimum (`Failed` -> `FailedResourceList`, `Pending` -> `PendingResourceList`, neither carrying a `length`), so emitting `[]` is correct for those two and wrong only for `Succeeded` — the handler treats all three identically. This crate elsewhere answers "nothing to report" by omitting the member rather than sending an empty list (`list_group_resources.rs:322` sends `query_errors: None`).

The grouping rule itself (report only the memberships THIS call added) is not in question and must not change; only the wire form of "it added none" is.

## Evidence

Model (smithy_query query="operation" operation="GroupResources"): output `"Succeeded": {"target": "ResourceArnList", "length": {"min": 1, "max": 10}, "type": "list"}`, beside `"Failed": {"target": "FailedResourceList", "type": "list"}` and `"Pending": {"target": "PendingResourceList", "type": "list"}`; and (smithy_query query="shape" shape="ResourceArnList") `"ResourceArnList": {"type": "list", "length": {"min": 1, "max": 10}, ...}` — no empty value is admitted.

Handler, services/service-com-amazonaws-resourcegroups-ardi/src/ops/group_resources.rs:262-267:
```
        let mut succeeded: Vec<String> = Vec::new();
        for arn in &input.resource_arns {
            if group.add_member(arn) {
                succeeded.push(arn.clone());
            }
        }
```
and group_resources.rs:287-293:
```
        Ok(GroupResourcesOutput {
            succeeded: Some(succeeded),
            // Nothing was refused, and a static membership this emulator accepts is
            // immediate — so neither list has an entry: an ARN the group already
            // held did not FAIL to be added, and none is left half-added.
            failed: Some(Vec::new()),
            pending: Some(Vec::new()),
```
So when every requested ARN fails `add_member` (already a member — `state.rs:1130` `return false;`), `succeeded` is `Some(vec![])`.

The handler's own comment states the reporting rule it derives this from, group_resources.rs:254-261: "Reported from what this call DID, not from the state it left behind: an ARN comes back in `Succeeded` because the group GAINED it here ... An ARN the group already held adds no membership, so it is reported in neither list" — correct as a rule, but `Succeeded` is the one member of the three the model forbids from being empty.

Same crate's pattern for "nothing to report", src/ops/list_group_resources.rs:320-322:
```
            // `QueryErrors` reports what went wrong while RUNNING a group's query;
            // listing recorded memberships runs none.
            query_errors: None,
```

Test coverage: tests/all/group_reported_additions.rs:167 and :219 read the list through the shared helper `succeeded` (defined at tests/all/ungroup_resources.rs:119-133), which already tolerates an absent member (`if answered["Succeeded"].is_null() { Vec::new() }`), so that suite keeps passing if the member is omitted. tests/all/group_resources_supported_configuration.rs:294-299 asserts `grouped["Succeeded"].as_array().map(Vec::len)` equals `Some(0)` for the repeated call, with the stated reason "Only the FIRST of the two calls adds a membership, so only it reports one — the repeated call adds nothing and reports nothing" — the reason is about reporting nothing, and that single assertion reads the array's LENGTH rather than its presence, so it is the one place that encodes the empty-list spelling.

## Steps to Reproduce

1. CreateGroup with a static-membership configuration: POST /groups {"Name":"members","Configuration":[{"Type":"AWS::EC2::HostManagement"}]}
2. GroupResources: POST /group-resources {"Group":"members","ResourceArns":["arn:aws:ec2:us-east-1:000000000000:instance/i-000000000000000a"]} -> 200, `Succeeded` carries that ARN (the group gained it)
3. Repeat the identical GroupResources call -> 200 with `"Succeeded": []`
4. Expected: no `Succeeded` member in the response, because the member targets `ResourceArnList`, modelled `length min 1, max 10`, which admits no empty list (`Failed` and `Pending` carry no minimum and may stay `[]`)
