Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# The Domains filter matches the caller's ARN byte-for-byte against an `arn:aws:`-only stored ARN, so a domain ARN in another modeled partition returns an empty page

_Filed: 2026-10-26T23:22:14Z_
_Operation: ListObjectives | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListObjectives` narrows to a domain with `objective.domain_arn == arn` (list_objectives.rs:230-234), where `arn` is the caller's `ObjectiveFilter.Domains[0].Arn` and `objective.domain_arn` is the ARN `catalog_arn` minted, which always hardcodes the `aws` partition (state.rs:282-284). The modeled `DomainArn` admits the whole `aws` partition family (`^arn:(aws(?:[-a-z]*)?):controlcatalog:::domain/[0-9a-z]+$`), so a filter naming a published domain in `aws-cn`, `aws-us-gov` or `aws-iso-b` passes the boundary validator, reaches the handler, matches no objective, and is answered 200 with an empty `Objectives` list — indistinguishable from "this domain has no objectives".

That contradicts the identity rule this crate states and implements for control ARNs: `state.resolve_control` (state.rs:191-203) deliberately ignores the `{PARTITION}` segment because "the catalog is global", and `GetControl` plus `ListControlMappings`' `ControlArns` filter are both tested across four partitions. The ontology filters skip that normalisation entirely, so the same catalog, reached through two of its own operations, answers two different ways about whether a partition segment is part of a catalog resource's identity.

`ListCommonControls` has the identical defect in the identical code shape — `common_control.objective_arn == arn` (list_common_controls.rs:239-243) against the modeled `ObjectiveArn` `^arn:(aws(?:[-a-z]*)?):controlcatalog:::objective/[0-9a-z]+$` — so one normalisation applied where the single-ARN resource filter is read closes both.

## Evidence

Raw comparison — services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_objectives.rs:227-234:
```
            state
                .objectives
                .iter()
                .filter(|(_, objective)| {
                    domain_filter
                        .as_deref()
                        .is_none_or(|arn| objective.domain_arn == arn)
                })
```
Sibling path in the same crate — services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_common_controls.rs:239-243:
```
                .filter(|(_, common_control)| {
                    objective_filter
                        .as_deref()
                        .is_none_or(|arn| common_control.objective_arn == arn)
                })
```
The stored ARN both compare against is minted in `aws` only — src/state.rs:282-284:
```
fn catalog_arn(kind: &str, id: &str) -> String {
    format!("arn:aws:controlcatalog:::{kind}/{id}")
}
```
The rule the crate states and implements for the other ARN kind — src/state.rs:186-190: "`{PARTITION}` says which partition the caller writes the ARN in and `{REGION}` where the control was enabled, so neither segment is part of a control's identity. An ARN of either form whose identifier the catalog publishes resolves, whatever partition it is written in — the catalog is global, and publishes one control per identifier."

The model admits the other partitions on exactly these members — smithy_query(query="shape", shape="DomainResourceFilter,ObjectiveResourceFilter"): `DomainResourceFilter.Arn` pattern `^arn:(aws(?:[-a-z]*)?):controlcatalog:::domain/[0-9a-z]+$`; `ObjectiveResourceFilter.Arn` pattern `^arn:(aws(?:[-a-z]*)?):controlcatalog:::objective/[0-9a-z]+$`.

Tests: the partition-insensitive half is pinned across four partitions — tests/all/control_arn_resolution.rs:28 `const PARTITIONS: [&str; 4] = ["aws", "aws-us-gov", "aws-cn", "aws-iso-b"];`, :70 `every_published_control_resolves_from_its_catalog_arn_in_any_partition`, :302 `the_control_arns_filter_resolves_a_control_from_any_accepted_arn`. The ontology filters are only ever driven with the ARN the service itself returned (tests/all/control_catalog.rs:462 `format!(r#"{{"ObjectiveFilter":{{"Domains":[{{"Arn":"{arn}"}}]}}}}"#)` and :550 `let absent_domain = format!("arn:aws:controlcatalog:::domain/{}", "z".repeat(25));`), so no test pins the cross-partition behaviour either way.

## Steps to Reproduce

1. POST /domains with `{}` and note a domain's `Arn`, e.g. `arn:aws:controlcatalog:::domain/<id>`.
2. POST /objectives with `{"ObjectiveFilter":{"Domains":[{"Arn":"arn:aws:controlcatalog:::domain/<id>"}]}}` — 200 with that domain's objectives.
3. POST /objectives with the same domain written in another partition the modeled `DomainArn` pattern admits: `{"ObjectiveFilter":{"Domains":[{"Arn":"arn:aws-cn:controlcatalog:::domain/<id>"}]}}` — 200 with an empty `Objectives` list, because list_objectives.rs:233 compares the ARN raw against the `arn:aws:` form state.rs:283 minted.
4. Repeat on /common-controls with `{"CommonControlFilter":{"Objectives":[{"Arn":"arn:aws-cn:controlcatalog:::objective/<id>"}]}}` — same empty 200 from list_common_controls.rs:242.
5. Contrast GetControl, which applies the crate's partition-insensitive rule: POST /get-control with `{"ControlArn":"arn:aws-cn:controlcatalog:::control/<published id>"}` returns the control (tests/all/control_arn_resolution.rs:70).
