Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5ba3ab4ab3bba3a3a0f46bab311929e6db691ea0

# UngroupResources answers an empty `Succeeded` list, which the modelled shape does not admit

_Filed: 2026-10-29T01:33:51Z_
_Operation: UngroupResources | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5ba3ab4ab3bba3a3a0f46bab311929e6db691ea0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When a request names only ARNs the group does not hold (the documented idempotent repeat of an ungrouping), the handler answers HTTP 200 with `"Succeeded": []`. The model targets that member at `ResourceArnList`, whose `@length` is `min: 1, max: 10`, so an empty list is not a value `Succeeded` can carry: the "nothing was removed" case has to be expressed by omitting the member, not by emitting a list the constraint excludes. The handler reached this shape by transplanting the reporting rule of the sibling tag removal onto a member the model constrains differently — `UntagOutput$Keys` (`TagKeyList`) carries no minimum, `Succeeded` does — and the handler's own comment states that analogy. Elsewhere in this crate a list with nothing to report is answered as absent rather than empty (`list_group_resources.rs:322` sends `query_errors: None`).

## Evidence

Model (smithy_query query="operation" operation="UngroupResources"): output `"Succeeded": {"target": "ResourceArnList", "length": {"min": 1, "max": 10}, "type": "list"}`; and `"ResourceArnList": {"type": "list", "length": {"min": 1, "max": 10}, ...}` — no empty value is admitted.

Handler, services/service-com-amazonaws-resourcegroups-ardi/src/ops/ungroup_resources.rs:241-246:
```
        let mut succeeded: Vec<String> = Vec::new();
        for arn in &input.resource_arns {
            if group.remove_member(arn) {
                succeeded.push(arn.clone());
            }
        }
```
and ungroup_resources.rs:266-267:
```
        Ok(UngroupResourcesOutput {
            succeeded: Some(succeeded),
```
So every ARN failing `remove_member` yields `Some(vec![])`.

The handler itself names the rule it copied, ungroup_resources.rs:236-237: "so it is reported in neither list, exactly as the sibling tag removal reports only the keys the stored map LOST" — but `UntagOutput$Keys` targets `TagKeyList`, which smithy_query shows with no `length` on the list at all, unlike `ResourceArnList`.

Same crate's pattern for "nothing to report", src/ops/list_group_resources.rs:320-322:
```
            // `QueryErrors` reports what went wrong while RUNNING a group's query;
            // listing recorded memberships runs none.
            query_errors: None,
```

Test coverage: tests/all/ungroup_resources.rs:119-126 reads either form ("A call that removed nothing reports no ARN, which the wire carries either as an empty list or as no member at all — both read here as no ARN"), so it does not pin the empty list and keeps passing if the member is omitted; tests/all/ungroup_reported_removals.rs:143-176 asserts only the ARNs the group LOST.

## Steps to Reproduce

1. CreateGroup with a static-membership configuration, e.g. `{"Name":"members","Configuration":[{"Type":"AWS::EC2::HostManagement"}]}`
2. GroupResources `{"Group":"members","ResourceArns":["arn:aws:ec2:us-east-1:000000000000:instance/i-000000000000000a"]}` -> 200, `Succeeded` carries the ARN
3. UngroupResources with the same `ResourceArns` -> 200, `Succeeded` carries the ARN (the group lost it)
4. UngroupResources a second time with the same `ResourceArns` -> 200 with `"Succeeded": []`; expected: no `Succeeded` member, because `ResourceArnList` is modelled `length min 1, max 10` and admits no empty list
