Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# The `externalId:` label-matching rule is reimplemented twice beside the state helper that documents itself as its ONE definition

_Filed: 2026-10-24T22:02:09Z_
_Operation: AssociateAssets | Class: maintainability_

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::matches_id_label` exists specifically to be the single definition of how a `CustomID` label (a raw id, or `externalId:` + external id) is matched against a stored `(id, externalId)` pair, and its doc comment says so in those words. `DescribeAssetProperty` and `ComAmazonawsIotsitewiseAwsiotsitewiseState::resolve_hierarchy_triple` both call it.

Two handlers nevertheless carry their own hand-rolled copy of the same three-line rule:
- `AssociateAssets` has a private `matches_label(hierarchy, label)`.
- `ListAssociatedAssets` has a private `resolve_hierarchy_label(declared, label)` whose filter closure inlines the identical `strip_prefix` / external-vs-id comparison.

Three copies of one rule can drift: a change to how the `externalId:` prefix is interpreted (or to how an empty stored id is treated) now has to be made in three places, and `AssociateAssets`/`ListAssociatedAssets` resolve the SAME hierarchy labels that `DisassociateAssets` resolves through the shared helper — so a drift would make associate and disassociate address different hierarchies. Routing both local functions through `crate::state::matches_id_label` changes no response: the predicate is byte-for-byte the same comparison, and the two call sites only differ in what they return around it (a `bool` vs the matched id).

## Evidence

The helper that claims to be the one definition — `src/state.rs:948-959`:

    /// Whether a stored `(id, externalId)` pair is the one a `CustomID` label names:
    /// the raw id, or `externalId:` followed by the external id.
    ///
    /// ONE definition of the two label forms, so every nested resource this service
    /// addresses by label — a hierarchy on an asset, a hierarchy on its model — is
    /// resolved the same way.
    pub fn matches_id_label(id: Option<&str>, external_id: Option<&str>, label: &str) -> bool {
        match label.strip_prefix(EXTERNAL_ID_PREFIX) {
            Some(external) => external_id == Some(external),
            None => id == Some(label),
        }
    }

Copy 1 — `src/ops/associate_assets.rs:381-388`:

    /// Whether `hierarchy` is the one a `CustomID` label names: the raw hierarchy id, or
    /// `externalId:` followed by the hierarchy's external id.
    fn matches_label(hierarchy: &AssetModelHierarchy, label: &str) -> bool {
        match label.strip_prefix(crate::state::EXTERNAL_ID_PREFIX) {
            Some(external) => hierarchy.external_id.as_deref() == Some(external),
            None => hierarchy.id.as_deref() == Some(label),
        }
    }

Copy 2 — `src/ops/list_associated_assets.rs:375-389`:

    fn resolve_hierarchy_label<'a>(
        declared: impl Iterator<Item = (Option<&'a str>, Option<&'a str>)>,
        label: &str,
    ) -> Option<String> {
        let external = label.strip_prefix(crate::state::EXTERNAL_ID_PREFIX);
        declared
            .filter(|(id, hierarchy_external)| match external {
                Some(wanted) => *hierarchy_external == Some(wanted),
                None => *id == Some(label),
            })
            .find_map(|(id, _)| id.map(str::to_string))
    }

Existing callers of the shared helper, showing it is already the crate's chosen seam: `src/ops/describe_asset_property.rs:252-256` (`crate::state::matches_id_label(property.id.as_deref(), property.external_id.as_deref(), &input.property_id,)`) and `src/state.rs:3333-3339` inside `resolve_hierarchy_triple`, which `DisassociateAssets` calls (`src/ops/disassociate_assets.rs:207-209`).

Tests that cover both copies and must stay green unchanged: `tests/all/asset_hierarchies.rs`, `tests/all/associated_assets.rs`, `tests/all/hierarchies_etags_and_refusals.rs`, `tests/all/asset_relationships.rs`, `tests/all/describe_asset_property.rs`.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/state.rs` at line 948 and read the doc comment claiming `matches_id_label` is the ONE definition of the two label forms.
2. Open `src/ops/associate_assets.rs` at line 383 — `matches_label` reimplements it for `AssetModelHierarchy`.
3. Open `src/ops/list_associated_assets.rs` at line 378 — `resolve_hierarchy_label`'s filter closure reimplements it a third time.
4. Note that `src/ops/describe_asset_property.rs:252` and `src/state.rs:3334` (the resolver `DisassociateAssets` uses) call the shared helper instead, so one rule is implemented in three places and the associate/disassociate pair resolves the same hierarchy labels through two of them.
