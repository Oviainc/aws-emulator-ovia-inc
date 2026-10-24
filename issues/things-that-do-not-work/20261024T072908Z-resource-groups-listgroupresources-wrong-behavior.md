Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5bfe48ddd26ea9415acafc303bdb706a28d090a8

# ListGroupResources reports only static memberships and never evaluates a group's ResourceQuery, so a tag-based or CloudFormation-stack group lists as empty

_Filed: 2026-10-24T07:29:08Z_
_Operation: ListGroupResources | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5bfe48ddd26ea9415acafc303bdb706a28d090a8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A resource group created with a `ResourceQuery` (TAG_FILTERS_1_0 or CLOUDFORMATION_STACK_1_0) derives its membership from that query: real ListGroupResources evaluates the query (hence its documented need for `tag:GetResources`, `cloudformation:DescribeStacks` and `cloudformation:ListStackResources` permissions) and returns the ARNs of the resources the query currently selects. The handler instead reads only `GroupRecord::members`, which is written exclusively by GroupResources (static membership). Expected: for a group whose query is `{"ResourceTypeFilters":[...],"TagFilters":[{"Key":"Stage","Values":["Test"]}]}`, ListGroupResources returns the resources carrying that tag (and QueryErrors for a stack identifier that cannot be read). Actual: 200 with `Resources`/`ResourceIdentifiers` empty and `QueryErrors` absent, no matter what the query says or what tagged resources exist in the emulator. The stored query is write-only state: nothing in the crate ever reads `ResourceQueryRecord::query` other than to echo it back from CreateGroup.

## Evidence

Spec: ListGroupResources is documented as 'Returns a list of Amazon resource names (ARNs) of the resources that are members of a specified resource group' and lists `tag:GetResources` / `cloudformation:ListStackResources` among the required permissions — i.e. membership is resolved by running the group's query; `ResourceQuery`/`QueryType` (TAG_FILTERS_1_0, CLOUDFORMATION_STACK_1_0) and the `QueryErrors` output member are modeled. Handler: `src/ops/list_group_resources.rs` lines ~255-285 builds the page from `group.members` only and hardcodes `query_errors: None` with the comment 'listing recorded memberships runs none'. `src/state.rs` stores `resource_query` but no code path evaluates it. Tests: `tests/all/resource_group_lifecycle.rs` only asserts static memberships added by `/group-resources`; no test asserts query-derived membership.

## Steps to Reproduce

1. POST /groups with {"Name":"tagged","ResourceQuery":{"Type":"TAG_FILTERS_1_0","Query":"{\"ResourceTypeFilters\":[\"AWS::EC2::Instance\"],\"TagFilters\":[{\"Key\":\"Stage\",\"Values\":[\"Test\"]}]}"}}.
2. Create/tag a matching resource in the emulator (e.g. an EC2 instance tagged Stage=Test).
3. POST /list-group-resources with {"Group":"tagged"}.
4. Observe 200 with an empty Resources/ResourceIdentifiers list instead of the ARN of the tagged resource.
