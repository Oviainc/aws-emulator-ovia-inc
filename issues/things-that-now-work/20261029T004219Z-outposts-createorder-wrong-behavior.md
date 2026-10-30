Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 51a9ae1c6292b29ea58f5661af821926d8567612

# A fulfilled order's line items never report AssetInformationList, although the crate installs exactly the assets that order's fulfilment delivered

_Filed: 2026-10-29T00:42:19Z_
_Operation: CreateOrder | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `51a9ae1c6292b29ea58f5661af821926d8567612`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateOrder` hardcodes `asset_information_list: None` on every `LineItem` it mints, and nothing ever fills it in afterwards — not `settle_orders`, which is the step that fulfils the order, and not the reads (`GetOrder`, `ListOrders`) that call it first. The justification written next to the hardcode is that the hardware installed for the item is something "this emulator records" nothing of. That premise is false inside this same crate: `crate::state::settle_orders` moves every line item to `LINE_ITEM_STATUS_INSTALLED` and, at the same instant, makes the Outpost's install due, which `settle_outposts` materialises by placing `installed_hardware(...)` — one `AssetInfo` per physical unit, each carrying a minted `AssetId` — on the Outpost the order was placed on. So at the moment the crate declares the line item INSTALLED it is also holding the asset ids of the hardware that install delivered, and the order still reports no asset information at all. The sibling member `OrderFulfilledDate` is handled the other way round (absent at create, written by `settle_orders` when the completion falls due), which is the pattern this member is missing.

No test pins the absence for a fulfilled order: `order_lifecycle.rs` asserts `AssetInformationList` absent only inside the just-submitted (`PREPARING`) create-response loop, where it is correct, and the same loop also asserts `OrderFulfilledDate` absent — a member the crate does populate once the order completes.

## Evidence

Hardcoded absent, with the premise it rests on — services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/create_order.rs:393-413:
```
// Each line item of the order gets its own minted `LineItemId`, and reports
// the catalog item and quantity the request asked for. What it does NOT
// report is anything that would be a claim about hardware: a
// `ShipmentInformation` is a carrier's tracking number for a unit in transit,
// an `AssetInformationList` is the hardware installed for the item, and the
// two `Previous…` members belong to a replacement order — none of which this
// request carries or this emulator records.
...
                    asset_information_list: None,
```

The same crate DOES record it — services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs:3886-3901 (`settle_orders`):
```
                for item in record.order.line_items.iter_mut().flatten() {
                    if item.status.as_deref() != Some(LINE_ITEM_STATUS_CANCELLED) {
                        item.status = Some(next.line_item_status.clone());
                    }
                }
...
                record.order.order_fulfilled_date = Some(timestamp(next.at));
                if let Some(outpost_id) = record.order.outpost_id.clone() {
                    installs.push((outpost_id, next.due));
                }
```
and state.rs:2480-2531 (`installed_hardware`): "Amazon Web Services installs the configuration the Outpost's form factor names once the order for it is fulfilled … so an Outpost whose ordered hardware has been installed holds one asset per physical unit of its configuration", building `AssetInfo { asset_id: Some(asset_id(sequence, position)), … }`.

The line-item status the install step writes — state.rs:1256-1260:
```
            OUTPOST_PROVISIONING,
            ORDER_STATUS_COMPLETED,
            LINE_ITEM_STATUS_INSTALLED,
```

Test coverage proving both halves are observable at once — services/service-com-amazonaws-outposts-outpostsolafservice/tests/all/outpost_order_driven_lifecycle.rs:381-396:
```
        let items = line_item_statuses(&order);
        assert_eq!(
            items,
            vec![LINE_ITEM_STATUS_INSTALLED.to_string(); row.line_items],
            "case {case:?}: every line item of a complete order is installed, got {order}"
        );
        // What was installed is the configuration the Outpost's form factor names, and
        // a form factor naming none installs nothing.
        let held = installed(&outposts, outpost_id).await;
```

The only test that pins the member absent does so for a just-submitted order — tests/all/order_lifecycle.rs:66-74 and 373-384: `assert_eq!(text(item, "Status"), LINE_ITEM_STATUS_PREPARING, …)` immediately followed by `for member in UNREPORTED_ON_ITEM { assert!(item.get(member).is_none(), …) }`; the same loop pins `OrderFulfilledDate` absent (order_lifecycle.rs:64, 330-336), a member `settle_orders` does write later.

## Steps to Reproduce

1. CreateSite, then CreateOutpost on it with SupportedHardwareType=RACK.
2. CreateOrder on that Outpost with one LineItem.
3. Wait past ORDER_PREPARATION + ORDER_DELIVERY + OUTPOST_PROVISIONING so the order's progression has fallen due.
4. ListAssets on the Outpost: it reports the installed assets, each with an AssetId (the install the order's fulfilment owed).
5. GetOrder on the order: Status is COMPLETED, OrderFulfilledDate is present, every LineItem.Status is INSTALLED — and every LineItem still reports no AssetInformationList, so nothing connects the installed assets to the line item that delivered them.
