Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c441cdb97fe099c25286b36cfc473da57b1ed5b6

# Read a page cursor's ARN through service-common's one ARN parse instead of this crate's own six-field split

_Filed: 2026-10-29T00:00:39Z_
_Operation: ListScheduledReports | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c441cdb97fe099c25286b36cfc473da57b1ed5b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::names_resource_arn` — the check every listing cursor is decoded through (`decode_page_token` -> `listing_page`, called by `ListScheduledReports` and `ListDashboards`) — re-implements the splitting of an ARN into its six colon-separated fields and the `<type>/<id>` split of the resource field. `service-common`, already a dependency of this crate, declares itself the single home of exactly that parse and exposes it as `arn::parse_of` plus `arn::resource_id_of_type`.

This is a pure de-duplication: the per-field GRAMMAR this service's ARN patterns state (the 20..2048 length, the `aws` partition prefix, the empty region, the twelve account digits, the `[-a-z0-9]`/`*` resource id) stays here and unchanged — only the field splitting and the type/id separation move onto the shared reader, so no response text, status, code or existing test changes.

## Evidence

The hand-rolled span, services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/state.rs:1676-1699:
```
pub fn names_resource_arn(arn: &str, resource_type: &str) -> bool {
    if !RESOURCE_ARN_LENGTH.contains(&arn.len()) {
        return false;
    }
    let fields: Vec<&str> = arn.split(':').collect();
    let &[prefix, partition, namespace, region, account, resource] = fields.as_slice() else {
        return false;
    };
    prefix == ARN_PREFIX_FIELD
        && partition
            .strip_prefix(ARN_PARTITION_PREFIX)
            .is_some_and(is_arn_label)
        && namespace == ARN_NAMESPACE
        ...
        && resource
            .strip_prefix(resource_type)
            .and_then(|rest| rest.strip_prefix('/'))
```

The crate that already provides it, services/common/src/arn.rs:12-17:
"Every service whose model binds a `resourceArn` the caller chooses has to read that field: an ARN of another vendor names nothing in this service's namespace, so it is a bad REQUEST ... so the one parse lives here rather than once per service."
and services/common/src/arn.rs:29-32:
"`service-com-amazonaws-emrserverless-awstoledowebservice/src/service.rs` still carries its own copy of this parse (as `arn_vendor`), inlined before this module existed ...; it is to be migrated onto this one rather than copied again."
The two readers that replace the split: `pub fn parse_of<'a>(value: &'a str, expected: &str) -> Option<Arn<'a>>` (services/common/src/arn.rs:167) and `pub fn resource_id_of_type<'a>(value: &'a str, resource_type: &str) -> Option<&'a str>` (services/common/src/arn.rs:150).

The dependency is already declared — services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/Cargo.toml:35: `service-common = { path = "../common" }`.

Existing tests that pin the behaviour this change must not alter: tests/all/listing_pagination.rs (cursor round-trip and refusal of a token this service never minted), tests/all/resource_not_found.rs:138-170 (the `absent_arns` table spanning partition, account id, the wildcard resource id and a 32-hex id) and tests/all/arn_minting.rs (the form of every minted ARN).

## Steps to Reproduce

1. Read services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/state.rs:1676-1699: the ARN is split into its six fields and the resource field is split on `/` by hand.
2. Read services/common/src/arn.rs:82-108, :150 and :167: `parse`/`parse_of` already return those six fields and `resource_id_of_type` already performs the `<type>/<id>` split, and the module header states this is the tree's single home for that parse.
3. Note services/.../Cargo.toml:35 already depends on `service-common`, so the shared reader is in reach with no new dependency.
4. Run the crate's tests (tests/all/listing_pagination.rs, resource_not_found.rs, arn_minting.rs) before and after routing the split through `service_common::arn`: every response and assertion is unchanged.
