Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f6f033d1876a0cb978cad56e1292384a0bfb4098

# Read a filter's control ARN through one helper: CanonicalFilter::read hand-rolls canonical_catalog_arn's resolve-or-keep-as-sent rule

_Filed: 2026-10-28T19:37:57Z_
_Operation: ListControlMappings | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f6f033d1876a0cb978cad56e1292384a0bfb4098`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate owns ONE rule for reading a caller-supplied catalog ARN before it is compared with a stored one: resolve the ARN to the ARN the catalog stores for the resource it names, and when it names nothing, keep it exactly as sent so it goes on matching nothing. That rule lives in `state::canonical_catalog_arn` (src/state.rs:330-334), and every list filter in the crate reaches it — `single_resource_filter_arn` for `ObjectiveFilter.Domains` and `CommonControlFilter.Objectives` (src/state.rs:445), and the `CommonControlArns` dimension of `ListControlMappings` (src/ops/list_control_mappings.rs:152).

Inside the SAME function, one branch above that call, the `ControlArns` dimension does not use it: it hand-rolls the rule as `state.resolve_control(requested).map_or_else(|| requested.clone(), |control| control.arn.clone())` (src/ops/list_control_mappings.rs:143-145), and restates the helper's documented fallback in its own prose ("An ARN that names nothing the catalog publishes is kept as sent, so it matches nothing", list_control_mappings.rs:119-120, against the helper's "An ARN that names no resource of `kind` is returned unchanged, so it goes on matching nothing", state.rs:327-328).

The control dimension genuinely needs `resolve_control` rather than `canonical_catalog_arn`, because a control may also be named by its Control Tower ARN form — but the resolve-or-keep canonicalisation on top of it, and the fact that the value produced is what BOTH the match (`matches`, list_control_mappings.rs:165-168) and the page fingerprint (`dimensions`, list_control_mappings.rs:193-205) are taken from, is the shared rule, now written in two places in two different idioms (`Option::map(..).unwrap_or_else(..)` in state.rs, `Option::map_or_else(..)` in the handler). The copies can drift: the crate has already been bitten twice by one ARN dimension being read by a different identity rule than its sibling (the fixed `20261026T232214Z-controlcatalog-listcontrolmappings-wrong-behavior`, where `CommonControlArns` compared the raw ARN while the `ControlArns` branch beside it resolved through the catalog), and this is the same seam left open in the opposite direction.

The fix is a `canonical_control_arn` on the state type (or a free function beside `canonical_catalog_arn`) that wraps `resolve_control` with the same resolve-or-keep contract, so the handler names only which dimension it is reading. Every response and every existing test stays unchanged, because the computed value is identical.

## Evidence

The helper that owns the rule — services/service-com-amazonaws-controlcatalog-controlcatalog/src/state.rs:327-334:
```
/// An ARN that names no resource of `kind` is returned unchanged, so it goes on
/// matching nothing: the modeled pattern on each member that accepts one refuses such
/// an ARN at the boundary anyway.
pub fn canonical_catalog_arn(kind: &str, requested_arn: &str) -> String {
    catalog_identifier(kind, requested_arn)
        .map(|identifier| catalog_arn(kind, identifier))
        .unwrap_or_else(|| requested_arn.to_string())
}
```

The same rule hand-rolled, one branch above the call that DOES use the helper — src/ops/list_control_mappings.rs:139-155:
```
        Self {
            control_arns: filter.control_arns.as_ref().map(|arns| {
                arns.iter()
                    .map(|requested| {
                        state
                            .resolve_control(requested)
                            .map_or_else(|| requested.clone(), |control| control.arn.clone())
                    })
                    .collect()
            }),
            common_control_arns: filter.common_control_arns.as_ref().map(|arns| {
                arns.iter()
                    .map(|requested| {
                        crate::state::canonical_catalog_arn("common-control", requested)
                    })
                    .collect()
            }),
```

The handler's own prose restating the helper's contract — src/ops/list_control_mappings.rs:115-122:
```
/// Each ARN dimension is rewritten to the ARN the catalog stores for the resource that
/// ARN names: neither the partition segment, nor the Region segment of the Control
/// Tower form of a control ARN, is part of a catalog resource's identity, so the
/// Control Tower form of a control's ARN and any partition's spelling of a catalog ARN
/// filter for the same resource. An ARN that names nothing the catalog publishes is
/// kept as sent, so it matches nothing. Both the match and the fingerprint a
/// `NextToken` carries are taken from these identities, so the same filter written two
/// ways narrows to one sequence.
```

The crate states elsewhere that the two kinds are one rule — src/state.rs:318-322 (doc on `canonical_catalog_arn`):
```
/// What names a catalog resource is its kind and the opaque identifier after
/// `{kind}/`, exactly as it is what names a control (see
/// [`ComAmazonawsControlcatalogControlcatalogState::resolve_control`]): the
/// `{PARTITION}` segment says which partition the caller writes the ARN in, and the
/// catalog is global, publishing one resource per kind and identifier.
```

Where the hand-rolled value is consumed, so a drift changes both the match and the token — src/ops/list_control_mappings.rs:165-168 and :193-196:
```
        if let Some(control_arns) = self.control_arns.as_ref() {
            if !control_arns.contains(&record.control_arn) {
                return false;
            }
```
```
    fn dimensions(&self) -> Vec<(&'static str, Vec<String>)> {
        vec![
            ("controlArns", self.control_arns.clone().unwrap_or_default()),
```

Test coverage that pins both copies, so the refactor is verifiable without new tests:
* tests/all/control_arn_resolution.rs:298-302 `the_control_arns_filter_resolves_a_control_from_any_accepted_arn` drives the hand-rolled copy in every accepted ARN form and partition.
* tests/all/control_mapping_filter_identity.rs:137 `both_arn_dimensions_of_one_filter_are_read_by_resource_identity`, :203 `the_mapping_type_dimension_narrows_the_same_filter_in_every_spelling`, :303 `a_mapping_page_token_resumes_across_the_spellings_of_its_filter` and :383 `an_identifier_naming_nothing_empties_the_conjunction_in_every_spelling` drive both copies together, including the `NextToken` the fingerprint issues and the "names nothing" fallback (`let unpublished = "9999999999999999999999999".to_string();`, control_mapping_filter_identity.rs:395).
* tests/all/catalog_resource_identity.rs covers `canonical_catalog_arn` itself for the domain, objective and common-control kinds.

## Steps to Reproduce

1. Open services/service-com-amazonaws-controlcatalog-controlcatalog/src/state.rs and read `canonical_catalog_arn` (lines 330-334) together with its doc at 316-329: resolve the requested ARN to the stored ARN for that kind, otherwise return the requested ARN unchanged so it matches nothing.
2. Open src/ops/list_control_mappings.rs and read `CanonicalFilter::read` (lines 132-158). Observe that the `common_control_arns` branch (lines 149-155) calls that helper, while the `control_arns` branch directly above (lines 140-148) spells the same resolve-or-keep rule out inline with `resolve_control(..).map_or_else(|| requested.clone(), ..)`.
3. Compare the two prose statements of the one rule: list_control_mappings.rs:119-120 ("An ARN that names nothing the catalog publishes is kept as sent, so it matches nothing") and state.rs:327-328 ("An ARN that names no resource of `kind` is returned unchanged, so it goes on matching nothing").
4. Note the drift surface: the value each branch produces feeds both `CanonicalFilter::matches` (list_control_mappings.rs:165-168) and `CanonicalFilter::dimensions` (lines 193-205), which is the fingerprint a `NextToken` carries — so changing the helper's fallback changes three filters and the common-control dimension of this one, but not the control dimension, which carries its own copy.
5. Confirm no helper for the control kind exists yet: `grep -n "canonical" src/state.rs src/ops/*.rs` lists only `canonical_catalog_arn` and its callers; `resolve_control` is reached raw from src/ops/get_control.rs:219 and src/ops/list_control_mappings.rs:144.
