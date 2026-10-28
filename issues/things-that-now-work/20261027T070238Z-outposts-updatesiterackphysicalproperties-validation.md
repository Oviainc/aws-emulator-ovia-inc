Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 75491f6dad17b9da11cd051dade27675220440ff

# UpdateSiteRackPhysicalProperties never refuses a rack update while an order at the site is in progress

_Filed: 2026-10-27T07:02:38Z_
_Operation: UpdateSiteRackPhysicalProperties | Class: validation_

> _Note: this issue was identified against the code base at commit `75491f6dad17b9da11cd051dade27675220440ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states a precondition for this operation: a rack at a site with an order of IN_PROGRESS cannot be updated until that order completes or is cancelled. The handler applies no such limit. It resolves the site, takes the store lock and writes every rack property the request carried, with no order ever consulted: the file contains no `settle_orders()` call and no reference to an order at all (the only occurrences of the word "order" in the whole file are the modelled doc sentence it reproduces and the boilerplate `ConflictException` doc). `ConflictException` (409) is a modelled error of this operation and the handler's generated error enum declares the variant, but the handler never constructs it, so the documented refusal can never be produced and the write always goes through (GetSite/ListSites then report the new properties).

The crate already owns both halves of the check this path skips: the sibling `UpdateSiteAddress` settles orders and then refuses with `ConflictException` when `state::site_address_block` reports an outstanding order at the site, and that block is built on `state::outstanding_order_at_site`, documented as "the order of `region` still OUTSTANDING at `site_id`". Nothing here calls either one, so the two site-mutating PATCH handlers disagree about whether an order at the site holds a change up.

## Evidence

Model sentence — smithy_query(query="doc", shape="UpdateSiteRackPhysicalProperties"): "Update the physical and logistical details for a rack at a site. ... To update a rack at a site with an order of IN_PROGRESS, you must wait for the order to complete or cancel the order." The handler reproduces the same sentence in its own module doc, services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/update_site_rack_physical_properties.rs:51-52: "To update a rack at a site with an order of IN_PROGRESS, you must wait for the order to complete or cancel the order." — while the handler's own IMPLEMENTED header (:4-9) claims only "a partial (PATCH) update of the stored site's `RackPhysicalProperties` ... Every rack property the request carries replaces the stored one".

What the handler actually does — update_site_rack_physical_properties.rs:229-243:
```
        let mut store = state.lock();
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
A grep of the file for `settle_|order|Conflict` returns only :51-52 (the modelled doc), :110 (the generated `ConflictException` doc line) and :134 (the unused `ConflictException(ConflictException)` enum variant) — no order is read and no 409 is ever built.

The sibling path that DOES apply the order limit — src/ops/update_site_address.rs:209-237:
```
        let mut store = state.lock();
        // Whether an order is still in progress depends on the status it holds NOW,
        // and an order moves on the clock, so the transition that was already due at
        // this instant is materialised before the statuses are read.
        store.settle_orders();
...
        if let Some(block) = store.site_address_block(&region, &site_id, kind) {
...
            return Err(UpdateSiteAddressErrorModeled::ConflictException(ConflictException {
```
and the helper it goes through — src/state.rs:3546-3561: "The order of `region` still OUTSTANDING at `site_id`, if this store holds one. ... so this is what the service still owes the SITE: the fact a change to either of that site's addresses is held for (see [`Self::site_address_block`])." with `pub fn outstanding_order_at_site(&self, region: &str, site_id: &str) -> Option<&OrderRecord>`.

Test coverage: tests/all/update_persistence.rs is the suite that pins this operation (:315-412 "The rule for `UpdateSiteRackPhysicalProperties`, over EVERY value EVERY one of the ..."), and a grep of that file for `/orders|CreateOrder|order` returns no matches — no case places an order, so nothing in the suite pins acceptance of a rack update while an order is in progress.

## Steps to Reproduce

1. POST /sites with {"Name":"rack-site"} -> note SiteId
2. POST /outposts with {"Name":"o","SiteId":"<SiteId>"} -> note OutpostId
3. POST /orders with {"OutpostIdentifier":"<OutpostId>","PaymentOption":"ALL_UPFRONT","LineItems":[{"CatalogItemId":"OR-ABCDEFG","Quantity":1}]} -> the order is PREPARING and then IN_PROGRESS on the clock (state::ORDER_PREPARATION), i.e. outstanding
4. PATCH /sites/<SiteId>/rackPhysicalProperties with {"PowerDrawKva":"POWER_5_KVA","UplinkGbps":"UPLINK_1G"} -> 200, and the answered Site carries the new RackPhysicalProperties
5. GET /sites/<SiteId> -> the new rack properties are persisted, although an order at the site is in progress; contrast PUT /sites/<SiteId>/address in the same state, which is refused 409 ConflictException
