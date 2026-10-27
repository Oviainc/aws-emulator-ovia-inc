Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# CommonControlArns filter compares the raw ARN while the ControlArns branch beside it resolves through the catalog, so a non-`aws` partition silently selects nothing

_Filed: 2026-10-26T23:22:14Z_
_Operation: ListControlMappings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Inside the single function `matches_filter` in `src/ops/list_control_mappings.rs`, the two ARN dimensions of `ControlMappingFilter` are matched by two different rules:

* `ControlArns` is resolved through `state.resolve_control(requested)` (list_control_mappings.rs:127-132), which the crate deliberately makes partition-insensitive: the identifier after `control/` names the control, and the `{PARTITION}` segment does not.
* `CommonControlArns` is matched with `common_control_arns.contains(common_control_arn)` (list_control_mappings.rs:140), i.e. byte equality against the stored ARN, which `catalog_arn` always mints in the `aws` partition (`format!("arn:aws:controlcatalog:::{kind}/{id}")`, state.rs:282-284).

The modeled `CommonControlArn` admits the whole `aws` partition family (`^arn:(aws(?:[-a-z]*)?):controlcatalog:::common-control/[0-9a-z]+$`), so a request whose filter names a published common control in `aws-cn`, `aws-us-gov` or `aws-iso-b` passes the boundary validator, reaches the handler, matches nothing, and is answered with HTTP 200 and an empty `ControlMappings` list — a successful-but-wrong answer for a common control the catalog does publish. The same request written with `arn:aws:` returns the mappings, and the very same call written in any of those partitions on the `ControlArns` dimension also returns them (pinned by the crate's own test). The crate therefore applies two contradictory identity rules to two ARNs in one filter.

The fix is to read a common-control ARN the way the crate already reads a control ARN — by the identifier the catalog publishes it under, independent of the `{PARTITION}` segment — so both dimensions of one filter obey one rule.

## Evidence

Both sides are in this crate.

Resolved (partition-insensitive) — services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_control_mappings.rs:127-132:
```
    if let Some(control_arns) = filter.control_arns.as_ref() {
        let matched = control_arns.iter().any(|requested| {
            state
                .resolve_control(requested)
                .is_some_and(|control| control.arn == record.control_arn)
        });
```
Raw (partition-sensitive), the next branch of the SAME function — list_control_mappings.rs:137-141:
```
    if let Some(common_control_arns) = filter.common_control_arns.as_ref() {
        let matched = match &record.mapping {
            crate::state::MappingDetails::CommonControl { common_control_arn } => {
                common_control_arns.contains(common_control_arn)
            }
```
The rule the crate states for the resolved side — src/state.rs:186-190: "`{PARTITION}` says which partition the caller writes the ARN in and `{REGION}` where the control was enabled, so neither segment is part of a control's identity. An ARN of either form whose identifier the catalog publishes resolves, whatever partition it is written in — the catalog is global, and publishes one control per identifier."

The stored ARN the raw branch compares against is always minted in `aws` — src/state.rs:282-284:
```
fn catalog_arn(kind: &str, id: &str) -> String {
    format!("arn:aws:controlcatalog:::{kind}/{id}")
}
```
The model admits the other partitions on exactly this member — smithy_query(query="shape", shape="CommonControlMappingDetails"): `CommonControlArn` pattern `^arn:(aws(?:[-a-z]*)?):controlcatalog:::common-control/[0-9a-z]+$` (same family on `ControlMappingFilter.CommonControlArns`).

Existing test coverage pins the resolved half and leaves the raw half untested cross-partition — tests/all/control_arn_resolution.rs:28 `const PARTITIONS: [&str; 4] = ["aws", "aws-us-gov", "aws-cn", "aws-iso-b"];` and tests/all/control_arn_resolution.rs:298-302: "The same rule holds wherever a control ARN is accepted: `ListControlMappings`' `ControlArns` filter resolves a control from either ARN form, in any partition, to the same mappings." — `the_control_arns_filter_resolves_a_control_from_any_accepted_arn` drives all four partitions through `ControlArns`; no test drives `CommonControlArns` with anything but the stored `arn:aws:` form (tests/all/control_mappings.rs:155 `every_mapping_kind_round_trips_through_its_own_filter` uses the ARN the service returned).

## Steps to Reproduce

1. POST /list-control-mappings with `{}` and note a COMMON_CONTROL mapping's `Mapping.CommonControl.CommonControlArn`, e.g. `arn:aws:controlcatalog:::common-control/<id>`.
2. POST /list-control-mappings with `{"Filter":{"CommonControlArns":["arn:aws:controlcatalog:::common-control/<id>"]}}` — 200 with that mapping returned.
3. POST /list-control-mappings with the same ARN written in another partition the modeled pattern admits: `{"Filter":{"CommonControlArns":["arn:aws-cn:controlcatalog:::common-control/<id>"]}}` — 200 with an empty `ControlMappings` list, because list_control_mappings.rs:140 compares the string raw.
4. Contrast the sibling dimension: POST /list-control-mappings with `{"Filter":{"ControlArns":["arn:aws-cn:controlcatalog:::control/<same control's id>"]}}` — 200 with the control's mappings, because list_control_mappings.rs:127-132 resolves through `state.resolve_control` (pinned by tests/all/control_arn_resolution.rs:302).
