Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0c701d5f8ae664c771f5c5cdfd2b2b982b891619

# UpdateSiteAddress never refuses an address change while an order is in progress

_Filed: 2026-10-25T04:07:26Z_
_Operation: UpdateSiteAddress | Class: validation_

> _Note: this issue was identified against the code base at commit `0c701d5f8ae664c771f5c5cdfd2b2b982b891619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents TWO limits on when a site address may change: (1) no address of either type may change while an order is in progress, and (2) the OPERATING address may change only before an order is placed at the site or after all Outposts of the site are deactivated. The handler implements only the second one — and only as "an Outpost record still names this site". It never looks at orders at all: it does not call `settle_orders()` and never consults `OrderRecord::is_outstanding`, the helper this same crate already uses for exactly this question in `blocking_resource_types`. Consequences:

* A SHIPPING_ADDRESS change is accepted unconditionally (`SiteAddressKind::Shipping => None` short-circuits the only block the handler has), even while an order placed on an Outpost of that site is still outstanding.
* An OPERATING_ADDRESS change is accepted as soon as no Outpost record names the site, even when an outstanding order for that site still exists (e.g. the order's Outpost was deleted while the order is still `PREPARING` / `IN_PROGRESS`).

In both cases the write goes through and `GetSiteAddress` / `GetSite` report the new address, so the documented refusal is never produced even though `ConflictException` (409) is a modelled error of this operation and the sibling decommission path already treats an outstanding order as a blocker.

## Evidence

Smithy doc for the operation — smithy_query(query="doc", shape="UpdateSiteAddress"): "Updates the address of the specified site." / "You can't update a site address if there is an order in progress. You must wait for the order to complete or cancel the order." / "You can update the operating address before you place an order at the site, or after all Outposts that belong to the site have been deactivated." The same sentence is reproduced in the handler's own module doc at services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/update_site_address.rs:55-57: "Updates the address of the specified site. You can't update a site address if there is an order in progress. You must wait for the order to complete or cancel the order."

What the handler actually checks — update_site_address.rs:205-226:
```
        // The Outpost that holds the site's OPERATING address, read before the
        // mutable borrow below. A shipping address is unaffected: the documented
        // limit is on the address the site itself is at.
        let blocking = match kind {
            crate::state::SiteAddressKind::Operating => store
                .outpost_at_site(&region, &site_id)
                .and_then(|record| record.outpost.outpost_id.clone()),
            crate::state::SiteAddressKind::Shipping => None,
        };
```
and then update_site_address.rs:231 `record.set_address(kind, input.address);` — no order is ever consulted, and `store.settle_orders()` is never called in this handler (grep of the file shows no `settle_` call at all). Its header only claims the second rule, update_site_address.rs:11-15: "The documented limit on WHEN the operating address may change — \"You can update the operating address before you place an order at the site, or after all Outposts that belong to the site have been deactivated\" — is the modelled `ConflictException` (409) while an Outpost still belongs to the site".

The crate already owns the "order in progress" predicate it skips — src/state.rs:1944-1948:
```
        let order_outstanding = all_in_region(&self.orders, region).any(|record| {
            record.order.outpost_id.as_deref() == Some(outpost_id) && record.is_outstanding()
        });
        if quote_ordered_from || order_outstanding {
            blocking.insert(BLOCKING_OUTPOST_ORDER_CANCELLABLE.to_string());
```
and the sibling handler calls it after settling: src/ops/start_outpost_decommission.rs:203-204 `store.settle_quotes(); store.settle_orders();` then :226 `let blocking = store.blocking_resource_types(&region, &outpost_id);`.

Test coverage: tests/all/site_address.rs:342-406 pins only the Outpost-at-site rule ("Rule 3: while an Outpost belongs to the site, a change to the OPERATING address is refused 409 `ConflictException` ... a change to the SHIPPING address goes through"); that test creates no order (tests/all/site_address.rs:347-361), so nothing in the suite pins the acceptance of an address change while an order is outstanding.

## Steps to Reproduce

1. POST /sites with {"Name":"s","OperatingAddress":{...}} -> note SiteId
2. POST /outposts with {"Name":"o","SiteId":"<SiteId>"} -> note OutpostId
3. POST /orders with {"OutpostIdentifier":"<OutpostId>","PaymentOption":"ALL_UPFRONT","LineItems":[{"CatalogItemId":"OR-ABCDEFG","Quantity":1}]} -> order is PREPARING/IN_PROGRESS, i.e. outstanding (OrderRecord::is_outstanding)
4. PUT /sites/<SiteId>/address with {"AddressType":"SHIPPING_ADDRESS","Address":{...new...}} -> 200, the new address is stored (GET /sites/<SiteId>/address?AddressType=SHIPPING_ADDRESS returns it), although an order is in progress
5. DELETE /outposts/<OutpostId> (the order stays outstanding and is still readable at GET /orders/<OrderId>)
6. PUT /sites/<SiteId>/address with {"AddressType":"OPERATING_ADDRESS","Address":{...new...}} -> 200 as well, with the order still in progress
