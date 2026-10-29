Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 9aa9958003004f44d89dbc31abda0f8624986a4f

# CreateOrder accepts a line item for a catalog configuration the crate itself declares is no longer offered (DISCONTINUED)

_Filed: 2026-10-29T07:47:13Z_
_Operation: CreateOrder | Class: validation_

> _Note: this issue was identified against the code base at commit `9aa9958003004f44d89dbc31abda0f8624986a4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state.rs` publishes a fixed catalog of hardware configurations and records a `CatalogItemStatus` on each one, declaring in its own doc comments that `AVAILABLE` is the status "a configuration that can still be ordered carries" and `DISCONTINUED` is the status of "a configuration that is no longer offered". One of the published configurations is seeded `DISCONTINUED` (state.rs:2161), so `GetCatalogItem` / `ListCatalogItems` really do report an item whose status says it cannot be ordered.

`CreateOrder` is the one operation of this service that orders a catalog configuration, and its handler never looks at `CatalogItemStatus` — it never resolves `LineItemRequest$CatalogItemId` against the catalog at all. The handler maps each requested line item straight onto a stored `LineItem`, minting a `LineItemId` and copying `requested.catalog_item_id` through verbatim (create_order.rs:400-415). The crate already owns the lookup the check needs (`ComAmazonawsOutpostsOutpostsolafserviceState::catalog_item`, state.rs:3159), and `get_catalog_item.rs:158-169` is the sibling that calls it and answers the modelled `NotFoundException` when it resolves nothing; `grep` shows `create_order.rs` is the only file under `src/ops/` that mentions `catalog_item_id` without ever calling that helper.

Consequence: an order for the DISCONTINUED SKU answers 200, the order record persists it, the clock then carries the order to `COMPLETED` and installs hardware on the Outpost — i.e. the emulator sells, fulfils and installs a configuration its own catalog declares is no longer offered. `CreateOrder` models `NotFoundException` (404) and `ValidationException` (400), so it has a modelled channel for the refusal; nothing in the handler or in a test states a reason for accepting a configuration whose published status says it cannot be ordered.

Note: the related case of a SKU the catalog publishes *nothing* under is deliberately accepted and is pinned by `tests/all/configured_instance_types.rs:481-487` with a stated reason, so this finding is specifically about the item the catalog DOES publish and marks DISCONTINUED — a status no handler reads anywhere in the crate.

## Evidence

The crate's own rule about what the two statuses mean — `src/state.rs:1976-1987`:

    /// The `CatalogItemStatus` a configuration that can still be ordered carries, from
    /// the modelled value set [`crate::ops::types::CATALOG_ITEM_STATUS_VALUES`]
    /// (`AVAILABLE` | `DISCONTINUED`).
    pub const CATALOG_ITEM_STATUS_AVAILABLE: &str = "AVAILABLE";

    /// The `CatalogItemStatus` a configuration that is no longer offered carries.
    ///
    /// A discontinued item stays IN the catalog — it is still what an Outpost already
    /// installed was ordered as — so a read of the catalog reports it; the operation
    /// models no status filter to leave it out with. See
    /// [`CATALOG_ITEM_STATUS_AVAILABLE`].
    pub const CATALOG_ITEM_STATUS_DISCONTINUED: &str = "DISCONTINUED";

The published configuration that carries it — `src/state.rs:2158-2169`:

    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_RACK,
        generation: OUTPOST_GENERATION_1,
        item_status: CATALOG_ITEM_STATUS_DISCONTINUED,

What `CreateOrder` does with the SKU the request names — `src/ops/create_order.rs:400-415` (the whole treatment of a line item; no catalog lookup, no status read):

            let line_items: Vec<LineItem> = line_items
                .into_iter()
                .map(|requested| {
                    let (line_item_id, _) = store.mint_id("ooi");
                    LineItem {
                        catalog_item_id: requested.catalog_item_id,
                        line_item_id: Some(line_item_id),
                        quantity: requested.quantity,
                        status: Some(crate::state::LINE_ITEM_STATUS_PREPARING.to_string()),

The helper the crate already has, and the sibling that uses it to refuse an unresolvable catalog item — `src/state.rs:3156-3163`:

    /// The hardware configuration this service publishes under the `SkuCode`
    /// `catalog_item_id`, or `None` for a code of that form it publishes nothing
    /// under — which this service holds no published hardware facts for.
    pub fn catalog_item(&self, catalog_item_id: &str) -> Option<&CatalogItemRecord> {

and `src/ops/get_catalog_item.rs:158-169`:

        let catalog_item = store
            .catalog_item(&input.catalog_item_id)
            .map(|record| record.item.clone())
            .ok_or_else(|| {
                GetCatalogItemErrorModeled::NotFoundException(NotFoundException {

The sibling create that DOES resolve every reference it is handed before persisting it — `src/ops/start_capacity_task.rs:275-279`:

        // The two OPTIONAL references a capacity task carries name other resources
        // of this service, so each is resolved against what the store holds before
        // anything is created: a reference that resolves to nothing is the not-found
        // error this operation models, worded as the identifier echo of the member it
        // arrived in, and no task is stored carrying it.

Test coverage today: `tests/all/configured_instance_types.rs:449-453` indexes the published catalog for its SKUs ("An index into the published catalog rather than a SKU spelled out, so a row names hardware this service really publishes") and `:481-487` asserts only that `UNPUBLISHED_SKU` is absent from the catalog; `grep -r item_status src/ops` returns nothing, so no handler and no test ever reads a catalog item's status when an order is placed.

## Steps to Reproduce

1. GET /catalog/items and find the item whose `ItemStatus` is `DISCONTINUED` (the generation-1 RACK configuration seeded at state.rs:2158-2169); note its `CatalogItemId`.
2. GET /catalog/item/{that CatalogItemId} — 200, confirming the item is published and DISCONTINUED.
3. POST /sites with {"Name":"s"}, then POST /outposts with that SiteId, {"Name":"o","SupportedHardwareType":"RACK"}.
4. POST /orders with {"OutpostIdentifier":"<op-…>","PaymentOption":"ALL_UPFRONT","LineItems":[{"CatalogItemId":"<the DISCONTINUED SKU>","Quantity":1}]} — answers 200 and persists the order, although the crate declares that configuration "no longer offered".
5. GET /orders/{OrderId} after the progression falls due: Status is COMPLETED and the Outpost reported by GET /outposts/{OutpostId} is ACTIVE with installed assets — hardware delivered from a configuration that can no longer be ordered.
