Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 51a9ae1c6292b29ea58f5661af821926d8567612

# A request carrying no rack property still gives a site an empty RackPhysicalProperties structure, against the handler's own comment and the test rule

_Filed: 2026-10-29T00:42:19Z_
_Operation: UpdateSiteRackPhysicalProperties | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `51a9ae1c6292b29ea58f5661af821926d8567612`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All nine rack properties this operation takes are optional in the model, so a PATCH carrying only the `SiteId` label is a valid request. The handler materialises `record.site.rack_physical_properties` with `get_or_insert_with(RackPhysicalProperties::default)` BEFORE any `crate::state::apply` call, so the insertion happens whether or not the request carried a property. On a site created with no rack properties at all, a no-op call therefore changes observable state: `GetSite` / `ListSites` / the update's own response go from omitting `RackPhysicalProperties` to reporting it as an empty object.

That contradicts two statements in this crate. The comment directly above the insertion says the structure appears "as soon as this call sets its first property" — but the code inserts it when the call sets none. And the test that owns the omitted-member rule states "one carrying none changes nothing"; it only exercises that case against a site that already has rack properties, so the observable change on a site without them is untested.

The crate recognises exactly this concern elsewhere and justifies the equivalent insertion only because it is unobservable: `state::tags_mut` materialises an absent tag map and the doc notes "That is not a change anybody can observe: [`tags_member`] already reports a resource with no tags as an empty map either way." For rack properties the opposite holds — absent vs. present-and-empty is observable, as the test's own assertion on a freshly created site proves.

## Evidence

The unconditional insertion, under the comment it contradicts — services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/update_site_rack_physical_properties.rs:268-283:
```
        let record = store.site_mut(&region, &site_id).ok_or_else(not_found)?;
        // A site created without rack properties at all gains the structure as soon
        // as this call sets its first property, so a property set only here is
        // reported rather than dropped.
        let properties = record
            .site
            .rack_physical_properties
            .get_or_insert_with(RackPhysicalProperties::default);
        // Every rack property `RackPhysicalProperties` declares, each written only
        // when the request carried it.
        crate::state::apply(&mut properties.power_draw_kva, input.power_draw_kva);
```
`apply` writes only when the request carried the member — src/state.rs:5097-5101:
```
pub fn apply<T>(stored: &mut Option<T>, update: Option<T>) {
    if let Some(value) = update {
        *stored = Some(value);
    }
}
```
so with an empty body the only write performed is the `get_or_insert_with`.

Every property is optional, so an empty body reaches the handler — smithy_query(query="operation", operation="UpdateSiteRackPhysicalProperties"): only `SiteId` carries `"required": true`; `PowerDrawKva`, `PowerPhase`, `PowerConnector`, `PowerFeedDrop`, `UplinkGbps`, `UplinkCount`, `FiberOpticCableType`, `OpticalStandard`, `MaximumSupportedWeightLbs` are all optional enums.

The test rule it breaks — tests/all/update_persistence.rs:355-357 and 379:
```
/// The same rule in its many-members-at-once and omitted-members forms, over a site
/// whose rack properties were set at creation: a request carrying several properties
/// writes exactly those, and one carrying none changes nothing.
...
        ("no property at all, by id", site_id.as_str(), json!({})),
```
and the assertion proving absent-vs-empty is observable — tests/all/update_persistence.rs:328-331:
```
    assert!(
        created.get("RackPhysicalProperties").is_none(),
        "a site created with no rack properties reports none, got {created:?}"
    );
```

The crate's own standard for an unconditional insertion — src/state.rs:4282-4300 (`tags_mut`): "The map is materialised when the stored record carries none … That is not a change anybody can observe: [`tags_member`] already reports a resource with no tags as an empty map either way."

## Steps to Reproduce

1. CreateSite with only {"Name": "rack site"} (no RackPhysicalProperties).
2. GetSite: the response has no RackPhysicalProperties member (pinned by update_persistence.rs:328-331).
3. PATCH /sites/{SiteId}/rackPhysicalProperties with an empty body {} — a valid request, since every rack property is optional.
4. The response, and every later GetSite / ListSites, now reports "RackPhysicalProperties": {} — the no-op call changed the site, while the handler comment says the structure appears only once the call sets its first property and update_persistence.rs:357 states a request carrying none changes nothing.
