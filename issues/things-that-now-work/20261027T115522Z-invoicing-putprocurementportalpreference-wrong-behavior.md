Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 16003bcc8481d509912583747e009ee504542034

# A Put that disables a plane leaves its verification status behind, so a disabled plane reports a status a create never gives it

_Filed: 2026-10-27T11:55:22Z_
_Operation: PutProcurementPortalPreference | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `16003bcc8481d509912583747e009ee504542034`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateProcurementPortalPreference` establishes the rule that a verification status belongs to an ENABLED plane only: it stores `pending(input.einvoice_delivery_enabled)` / `pending(input.purchase_order_retrieval_enabled)`, so a plane the create leaves disabled carries no status at all.

`PutProcurementPortalPreference` applies half of that rule. It fills a status in when the request ENABLES a plane that has none, but when the request DISABLES a plane it leaves the stored status untouched — the handler writes `preference.einvoice_delivery_enabled = input.einvoice_delivery_enabled` and then only ever adds a status, never clears one. The result is that two preferences with identical configuration report different state depending on history: one created with `EinvoiceDeliveryEnabled: false` reports no `EinvoiceDeliveryPreferenceStatus`, while one created enabled and then Put disabled reports `EinvoiceDeliveryEnabled: false` together with `EinvoiceDeliveryPreferenceStatus: PENDING_VERIFICATION`. `GetProcurementPortalPreference` and `ListProcurementPortalPreferences` both surface that stale status (`to_summary` copies it verbatim), so a client reading the resource cannot tell the two configurations apart by the members the preference reports.

## Evidence

The rule the create states and enforces — services/service-com-amazonaws-invoicing-invoicing/src/ops/create_procurement_portal_preference.rs:312-318 and :351-355:
```
        // Each plane starts PENDING_VERIFICATION, and only a plane this request ENABLES
        // has a verification status at all: the portal connection for an enabled plane
        // has not been verified yet, …
        let pending =
            |enabled: bool| enabled.then(|| crate::state::STATUS_PENDING_VERIFICATION.to_string());
…
            einvoice_delivery_preference_status: pending(input.einvoice_delivery_enabled),
…
            purchase_order_retrieval_preference_status: pending(
                input.purchase_order_retrieval_enabled,
            ),
```
The same rule applied one-way-only by the Put — src/ops/put_procurement_portal_preference.rs:252-311:
```
            preference.einvoice_delivery_enabled = input.einvoice_delivery_enabled;
            preference.purchase_order_retrieval_enabled = input.purchase_order_retrieval_enabled;
…
            if preference.einvoice_delivery_enabled
                && preference.einvoice_delivery_preference_status.is_none()
            {
                preference.einvoice_delivery_preference_status =
                    Some(crate::state::STATUS_PENDING_VERIFICATION.to_string());
            }
```
— there is no branch for the `!enabled` direction anywhere in the file, so a disable keeps whatever status the plane held.

The stale status is reported by both readers — src/ops/get_procurement_portal_preference.rs:180-182 (`procurement_portal_preference: stored.preference.clone()`) and src/state.rs:606-611 (`einvoice_delivery_preference_status: preference.einvoice_delivery_preference_status.clone()` in `to_summary`).

No test pins the disable direction: tests/all/procurement_portal_preference_lifecycle.rs:372-392 asserts the enabled case only —
```
            // A plane the update leaves enabled still carries the status the validation
            // calls move; a status is never invented for a plane that has none.
…
                if reported.get(enabled).and_then(Value::as_bool) == Some(true) {
                    assert_eq!(
                        reported.get(status).and_then(Value::as_str),
                        Some(PENDING_VERIFICATION),
```
so nothing states a reason for the asymmetry.

## Steps to Reproduce

1. CreateProcurementPortalPreference with `EinvoiceDeliveryEnabled: true` and `PurchaseOrderRetrievalEnabled: true`; note the ARN.
2. GetProcurementPortalPreference on that ARN: both planes report `PENDING_VERIFICATION`.
3. PutProcurementPortalPreference on the same ARN with `EinvoiceDeliveryEnabled: false`, `PurchaseOrderRetrievalEnabled: false` and the required `Contacts`.
4. GetProcurementPortalPreference again: the preference reports `EinvoiceDeliveryEnabled: false` / `PurchaseOrderRetrievalEnabled: false` but still carries `EinvoiceDeliveryPreferenceStatus` and `PurchaseOrderRetrievalPreferenceStatus` of `PENDING_VERIFICATION`.
5. CreateProcurementPortalPreference for a second buyer/supplier identity with both flags `false` and Get it: the same configuration reports NO status members — two identical configurations answered two different ways, and ListProcurementPortalPreferences repeats the divergence in its summaries.
