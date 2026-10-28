Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: afab077872ee6c32a9626f154d3b33fea96b8a18

# GroupResources reports already-present members as added in Succeeded

_Filed: 2026-10-28T18:16:09Z_
_Operation: GroupResources | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `afab077872ee6c32a9626f154d3b33fea96b8a18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GroupResources` builds its `Succeeded` list from "every requested ARN the group holds AFTER the call" rather than from the ARNs this call actually added. An ARN that was already a static member before the request is therefore reported in `Succeeded`, although this operation added nothing for it.

The model defines `Succeeded` as the resources this OPERATION added. The handler already computes the set of ARNs that were NOT members (`added`, used for the membership-ceiling check, and `added_count`, used for the audit record) one statement earlier — so the correct set is in scope and a different one is reported.

## Evidence

Smithy model, `GroupResourcesOutput$Succeeded` (`smithy_query(query="doc",shape="GroupResourcesOutput",member="Succeeded")`):

> "A list of Amazon resource names (ARNs) of the resources that this operation successfully added to the group."

Handler, `services/service-com-amazonaws-resourcegroups-ardi/src/ops/group_resources.rs:249-288` — the set of ARNs that this call will really add is computed, then only counted, and `Succeeded` is computed from post-call presence instead:

```
        let added: std::collections::BTreeSet<&str> = input
            .resource_arns
            .iter()
            .map(String::as_str)
            .filter(|arn| {
                group
                    .members
                    .binary_search_by(|member| member.resource_arn.as_str().cmp(arn))
                    .is_err()
            })
            .collect();
...
        let added_count = added.len();
        for arn in &input.resource_arns {
            group.add_member(arn);
        }
        // Reported from the memberships the group now holds, not from the request:
        // an ARN appears in `Succeeded` because it IS a member. Looked up by binary
        // search over the sorted memberships, so this too is bounded by
        // `MAX_RESOURCE_ARNS` lookups.
        let succeeded: Vec<String> = input
            .resource_arns
            .iter()
            .filter(|arn| {
                group
                    .members
                    .binary_search_by(|member| member.resource_arn.as_str().cmp(arn))
                    .is_ok()
            })
            .cloned()
            .collect();
```

The comment states the criterion outright — "an ARN appears in `Succeeded` because it IS a member" — which is not the criterion the model gives ("that this operation successfully added"). The handler's own module doc at `group_resources.rs:6-7` likewise says "reports the memberships that now hold in `Succeeded`. Adding an ARN that is already a member is a no-op".

Test coverage: `services/service-com-amazonaws-resourcegroups-ardi/tests/all/group_resources_supported_configuration.rs` and `tests/all/ungroup_resources.rs` exercise the grouping path.

## Steps to Reproduce

1. Create a static group: POST /groups with {"Name":"static-group","Configuration":[{"Type":"AWS::ResourceGroups::ApplicationGroup"}]}
2. POST /group-resources with {"Group":"static-group","ResourceArns":["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"]} — `Succeeded` correctly lists the one ARN added
3. POST the identical /group-resources request again
4. Observe `Succeeded` again lists that ARN, although this second call added nothing (the handler's own `added_count` is 0 for it); per the model `Succeeded` is "the resources that this operation successfully added to the group"
