Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5bfe48ddd26ea9415acafc303bdb706a28d090a8

# GroupResources adds static members to any group, including query-based groups the operation does not support

_Filed: 2026-10-24T07:29:08Z_
_Operation: GroupResources | Class: validation_

> _Note: this issue was identified against the code base at commit `5bfe48ddd26ea9415acafc303bdb706a28d090a8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation is documented as usable with only three group configuration types — `AWS::EC2::HostManagement`, `AWS::EC2::CapacityReservationPool`, `AWS::ResourceGroups::ApplicationGroup` — and 'Other resource group types and resource types are not currently supported by this operation'. Real Resource Groups therefore refuses a GroupResources call against a tag-query group (or any group with none of those configuration types) with BadRequestException (HTTP 400). The handler performs no such check: it resolves the group by name or ARN and calls `add_member` for every ARN, so a group created with `ResourceQuery: {Type: TAG_FILTERS_1_0}` — or with no query and no configuration at all — happily accumulates static memberships and reports them all in `Succeeded`. Expected: 400 BadRequestException when the addressed group's stored configuration is not one of the three supported types. Actual: 200 with every ARN in `Succeeded` and the memberships persisted, observable through ListGroupResources.

## Evidence

Spec: GroupResources documentation (smithy_query doc=GroupResources) — 'You can only use this operation with the following groups: AWS::EC2::HostManagement, AWS::EC2::CapacityReservationPool, AWS::ResourceGroups::ApplicationGroup. Other resource group types and resource types are not currently supported by this operation.' Handler: `src/ops/group_resources.rs` lines ~207-253 — the only refusals are list length, ARN shape, group-not-found and the emulator's own per-group ceiling; `record.configuration` is never consulted. Tests: `tests/all/resource_group_lifecycle.rs` adds members to groups created with no configuration at all (e.g. `{"Name":"members"}`) and asserts success, so no test pins the documented restriction.

## Steps to Reproduce

1. POST /groups with {"Name":"tagq","ResourceQuery":{"Type":"TAG_FILTERS_1_0","Query":"{\"TagFilters\":[{\"Key\":\"Stage\",\"Values\":[\"Test\"]}]}"}}.
2. POST /group-resources with {"Group":"tagq","ResourceArns":["arn:aws:ec2:us-east-1:000000000000:instance/i-000000000000000a"]}.
3. Observe 200 with the ARN in Succeeded instead of 400 BadRequestException.
4. POST /list-group-resources with {"Group":"tagq"} and observe the static membership was persisted.
