Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5bfe48ddd26ea9415acafc303bdb706a28d090a8

# ListGroups answers 501 for the modeled configuration-type and resource-type filters instead of filtering

_Filed: 2026-10-24T07:29:08Z_
_Operation: ListGroups | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5bfe48ddd26ea9415acafc303bdb706a28d090a8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GroupFilterName` declares five filter names: `resource-type`, `configuration-type`, `owner`, `display-name`, `criticality`. The handler answers only the last three; for `configuration-type` and `resource-type` it returns `Scaffolded::not_implemented`, i.e. HTTP 501, for an otherwise valid request. Expected: 200 with the groups whose configuration type (for `configuration-type`, e.g. `AWS::EC2::HostManagement`) or whose query resource types (for `resource-type`) match the filter values. Actual: 501 NotImplemented. `configuration-type` is answerable purely from state the emulator already persists — `GroupRecord::configuration` holds the `GroupConfigurationItem.Type` values set by CreateGroup — so this filter silently refuses a request the stored state can satisfy; `resource-type` additionally needs query evaluation. A 501 on an implemented operation also breaks clients/CLI paginators that pass a filter.

## Evidence

Spec: model enum `com.amazonaws.resourcegroups#GroupFilterName` values = [resource-type, configuration-type, owner, display-name, criticality]; ListGroups is modeled with `Filters: list<GroupFilter>` and 200 success. Handler: `src/ops/list_groups.rs` lines 190-211 match `configuration-type` | `resource-type` and return `crate::scaffolded::Scaffolded::not_implemented(...)`. Tests: `tests/all/resource_group_lifecycle.rs` lines 937-950 assert the 501 for both names, so the current behaviour is pinned by a test that encodes the gap rather than AWS behaviour.

## Steps to Reproduce

1. POST /groups with {"Name":"hosts","Configuration":[{"Type":"AWS::EC2::HostManagement"}]}.
2. POST /groups-list with {"Filters":[{"Name":"configuration-type","Values":["AWS::EC2::HostManagement"]}]}.
3. Observe HTTP 501 instead of 200 listing the group just created.
4. Repeat with {"Filters":[{"Name":"resource-type","Values":["AWS::EC2::Instance"]}]} and observe 501.
