Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: dbe9ca7a8a80909c811e91f1b58fc3cbb14634ed

# ListGroupResources never produces the documented BadRequestException for resource-type filter values outside the group's query

_Filed: 2026-10-25T04:30:41Z_
_Operation: ListGroupResources | Class: validation_

> _Note: this issue was identified against the code base at commit `dbe9ca7a8a80909c811e91f1b58fc3cbb14634ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The modelled documentation of `ListGroupResourcesInput$Filters` states a concrete error trigger: when a `resource-type` filter names types that are not part of the query associated with the group, the request must answer `BadRequestException` ("The resource types specified as filters in the request are not valid."), except when the group's query specifies `AWS::AllSupported`. The handler only checks the filter NAME and then filters the stored static memberships by their derived type; it never compares the filter's values against the addressed group's stored query, so a request the model says must be refused is answered with HTTP 200 and (for a query-based group, which holds no static memberships) an empty page — an error turned into a success. The group's query document is stored verbatim on the record (`GroupRecord::resource_query`), and the crate already parses such documents in `crate::state::check_resource_query`, so the data needed for the documented refusal is in hand.

## Evidence

Model (smithy_query doc, ListGroupResourcesInput$Filters): "When you specify a `resource-type` filter for `ListGroupResources`, Resource Groups validates your filter resource types against the types that are defined in the query associated with the group. … In this case, a `ListGroupResources` request returns a `BadRequestException` error with a message similar to the following: `The resource types specified as filters in the request are not valid.` … This validation doesn't occur when the group query specifies `AWS::AllSupported`".

Handler, src/ops/list_group_resources.rs:216-225 — only the filter NAME is judged:
```
        let filters = input.filters.unwrap_or_default();
        for filter in &filters {
            if filter.name != "resource-type" {
                return Err(bad_request(format!(
                    "Filters contains the filter name {}, which is not valid: expected one of {}",
```
and src/ops/list_group_resources.rs:259-263 + 328-333 — the values are only used to filter stored memberships, never validated against the group's query:
```
        let mut remaining: Vec<&crate::state::MemberRecord> = group
            .members
            .iter()
            .filter(|member| filters.iter().all(|filter| matches_filter(member, filter)))
```
```
fn matches_filter(member: &crate::state::MemberRecord, filter: &ResourceFilter) -> bool {
    member
        .resource_type
        .as_ref()
        .is_some_and(|resource_type| filter.values.contains(resource_type))
```
The stored query is available: src/state.rs:480 `pub resource_query: Option<ResourceQueryRecord>,` and src/state.rs:397 parses a query document with `serde_json::from_str`.

## Steps to Reproduce

1. CreateGroup Name=s3only with ResourceQuery={"Type":"TAG_FILTERS_1_0","Query":"{\"ResourceTypeFilters\":[\"AWS::S3::Bucket\"],\"TagFilters\":[]}"} — 200.
2. ListGroupResources Group=s3only with Filters=[{"Name":"resource-type","Values":["AWS::EC2::Instance"]}].
3. Actual: HTTP 200 with an empty Resources/ResourceIdentifiers list. Expected per the quoted member documentation: BadRequestException (HTTP 400), because EC2 instances are not part of the group's query and the query does not specify AWS::AllSupported.
