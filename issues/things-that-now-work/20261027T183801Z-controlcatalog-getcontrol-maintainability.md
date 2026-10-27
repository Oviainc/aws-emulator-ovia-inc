Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 086f796f1e6ea795d98417e1d50d205591b703fd

# Read a global catalog ARN's opaque identifier through one helper: ControlArnRef::parse re-implements catalog_identifier

_Filed: 2026-10-27T18:38:01Z_
_Operation: GetControl | Class: maintainability_

> _Note: this issue was identified against the code base at commit `086f796f1e6ea795d98417e1d50d205591b703fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states ONE rule for reading a catalog ARN: a global `controlcatalog` ARN (service segment `controlcatalog`, empty Region segment) names the resource by the opaque identifier after `{kind}/`, and the `{PARTITION}` segment carries no identity. That rule is implemented TWICE in `src/state.rs`, within 120 lines of each other:

* `catalog_identifier(kind, arn)` (state.rs:334-345) — the generic helper, reached by every list handler through `canonical_catalog_arn` (`single_resource_filter_arn` for `ObjectiveFilter.Domains` / `CommonControlFilter.Objectives`, and `CommonControlArns` in `ListControlMappings`).
* `ControlArnRef::parse`'s `Catalog` branch (state.rs:220-232) — the same three checks spelled out inline (parse, strip `control/`, reject empty, require service `controlcatalog` with an empty Region), reached by `GetControl` and by `ListControlMappings`' `ControlArns` dimension through `ComAmazonawsControlcatalogControlcatalogState::resolve_control`.

For `kind == "control"` the two compute exactly the same value, so the inline copy adds nothing but a drift surface: the two differ already in the ORDER of their checks (the inline copy strips `control/` before it looks at the service segment, the helper checks the service and Region first), so a future edit to the helper's notion of "a global catalog ARN" — a Region rule, a second accepted namespace, a case rule — silently applies to the four list filters and not to `GetControl`, which is precisely the split the crate's two already-fixed cross-partition issues (`20261026T232214Z-controlcatalog-listobjectives-wrong-behavior`, `20261026T232214Z-controlcatalog-listcontrolmappings-wrong-behavior`) were about.

The fix is for `ControlArnRef::parse`'s `Catalog` branch to call `catalog_identifier("control", arn)` instead of restating it, leaving `ControlArnRef` to own only the part that is genuinely its own: the `controltower` namespace. Every response and every existing test stays unchanged.

## Evidence

Copy 1 — the generic helper, services/service-com-amazonaws-controlcatalog-controlcatalog/src/state.rs:332-345:
```
/// The opaque identifier a `controlcatalog` ARN of `kind` carries, or `None` when the
/// ARN names no resource of that kind.
fn catalog_identifier<'a>(kind: &str, arn: &'a str) -> Option<&'a str> {
    let parsed = AccountlessArn::parse(arn)?;
    // The catalog is global, so its own ARNs carry no Region segment.
    if parsed.service != "controlcatalog" || !parsed.region.is_empty() {
        return None;
    }
    let identifier = parsed
        .resource
        .strip_prefix(kind)
        .and_then(|rest| rest.strip_prefix('/'))?;
    (!identifier.is_empty()).then_some(identifier)
}
```

Copy 2 — the same rule restated inline, state.rs:218-233:
```
impl<'a> ControlArnRef<'a> {
    /// Read an ARN as the control it names, or `None` when it names none.
    fn parse(arn: &'a str) -> Option<Self> {
        let parsed = AccountlessArn::parse(arn)?;
        let identifier = parsed.resource.strip_prefix("control/")?;
        if identifier.is_empty() {
            return None;
        }
        match parsed.service {
            // The global catalog form carries no Region segment.
            "controlcatalog" if parsed.region.is_empty() => Some(Self::Catalog(identifier)),
            "controltower" => Some(Self::ControlTower(identifier)),
            _ => None,
        }
    }
}
```

The crate itself says the two are the same rule — state.rs:312-321, the doc on `canonical_catalog_arn`:
```
/// What names a catalog resource is its kind and the opaque identifier after
/// `{kind}/`, exactly as it is what names a control (see
/// [`ComAmazonawsControlcatalogControlcatalogState::resolve_control`]): the
/// `{PARTITION}` segment says which partition the caller writes the ARN in, and the
/// catalog is global, publishing one resource per kind and identifier.
```

The two call sites that diverge at the copies — state.rs:191-195 (`GetControl`'s path):
```
    pub fn resolve_control(&self, requested_arn: &str) -> Option<&Control> {
        match ControlArnRef::parse(requested_arn)? {
            ControlArnRef::Catalog(identifier) => {
                self.controls.get(&catalog_arn("control", identifier))
            }
```
and state.rs:326-330 (the list filters' path):
```
pub fn canonical_catalog_arn(kind: &str, requested_arn: &str) -> String {
    catalog_identifier(kind, requested_arn)
        .map(|identifier| catalog_arn(kind, identifier))
        .unwrap_or_else(|| requested_arn.to_string())
}
```

Test coverage that pins both copies, so the refactor is verifiable without new tests:
* `tests/all/control_arn_resolution.rs:70` `every_published_control_resolves_from_its_catalog_arn_in_any_partition` and `:199` `a_control_tower_arn_resolves_in_any_partition_and_region` drive both branches of `ControlArnRef::parse` across `const PARTITIONS: [&str; 4] = ["aws", "aws-us-gov", "aws-cn", "aws-iso-b"];` (control_arn_resolution.rs:28); `:240` `an_unpublished_identifier_is_not_found_in_every_partition` drives its `None` results, including "a catalog identifier asked for in the Control Tower namespace".
* `tests/all/catalog_resource_identity.rs:72` `the_objective_filter_resolves_a_domain_in_any_partition`, `:114` `the_common_control_filter_resolves_an_objective_in_any_partition`, `:161` `the_mapping_filter_resolves_a_common_control_in_any_partition` and `:217` `an_identifier_the_catalog_does_not_publish_selects_nothing_in_any_partition` drive `catalog_identifier` for all three kinds.

## Steps to Reproduce

1. Open services/service-com-amazonaws-controlcatalog-controlcatalog/src/state.rs and read `catalog_identifier` (lines 332-345): parse the ARN, require service `controlcatalog` with an empty Region segment, strip `{kind}/`, reject an empty identifier.
2. Read `ControlArnRef::parse` (lines 218-233) and observe the identical four steps for `kind == "control"`, spelled inline and in a different order (resource prefix first, service/Region second).
3. Confirm the two are reached by different operations: `GetControl` -> `resolve_control` -> `ControlArnRef::parse` (src/ops/get_control.rs:219 `state.resolve_control(&input.control_arn)`), while `ListObjectives`/`ListCommonControls`/`ListControlMappings` reach `catalog_identifier` through `canonical_catalog_arn` (src/ops/list_control_mappings.rs:152, src/state.rs:431).
4. Note the drift surface: editing the helper's definition of a global catalog ARN changes the four list filters but not `GetControl`, because `GetControl` reads its own copy.
