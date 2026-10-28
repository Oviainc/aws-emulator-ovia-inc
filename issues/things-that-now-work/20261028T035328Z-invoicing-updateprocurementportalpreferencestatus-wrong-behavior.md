Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f84dda0f3deaf97a2c7ee51f672a37b240346b3

# UpdateProcurementPortalPreferenceStatus writes a status onto a plane the configuration leaves disabled, which the next configuration write silently erases

_Filed: 2026-10-28T03:53:28Z_
_Operation: UpdateProcurementPortalPreferenceStatus | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f84dda0f3deaf97a2c7ee51f672a37b240346b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states the verification state a preference's planes may report in ONE place, `crate::state::apply_plane_verification_statuses`, whose doc says a plane the configuration leaves DISABLED reports no status and no reason at all, "because there is no connection for either to be about", and that this is "one rule about the RESOURCE ... so two preferences holding the same configuration cannot report different state because they reached it by different paths". `CreateProcurementPortalPreference` and `PutProcurementPortalPreference` both apply that rule.

`UpdateProcurementPortalPreferenceStatus` assigns the two `Option<String>` pairs directly and never consults `einvoice_delivery_enabled` / `purchase_order_retrieval_enabled`, so it is the one write that can leave the resource in the state the rule says cannot exist: a preference created with `EinvoiceDeliveryEnabled: false` (status absent) answers 200 to a request stating `EinvoiceDeliveryPreferenceStatus: "ACTIVE"`, and `GetProcurementPortalPreference` / `ListProcurementPortalPreferences` then report `EinvoiceDeliveryEnabled: false` together with `EinvoiceDeliveryPreferenceStatus: "ACTIVE"`.

The written state is also unstable: the very next `PutProcurementPortalPreference` that restates that plane as disabled runs `apply_plane_verification_statuses` and silently clears the status and reason the earlier 200 persisted — while the preference's `Version` still counts the status write. Two preferences with identical configurations therefore report different verification state depending on the order of calls that reached it, which is exactly what the rule's doc says cannot happen.

The crate's two docs also disagree about whether this operation is a configuration write: `src/state.rs:44-48` counts "the status update" among "the calls that SUCCESSFULLY write a preference's configuration" (and the handler does bump `Version`/`LastUpdateDate` accordingly), while `apply_plane_verification_statuses` scopes the plane rule to "the create and the update" only — so the handler takes one half of the configuration-write contract and skips the other.

## Evidence

The rule, in this crate — services/service-com-amazonaws-invoicing-invoicing/src/state.rs:636-656:
```
/// The verification state each plane of a procurement portal preference reports after a
/// write of that preference's configuration, in ONE place.
///
/// ... a plane the configuration
/// enables and nothing has verified yet reports [`STATUS_PENDING_VERIFICATION`], and a
/// plane the configuration leaves DISABLED reports no status and no reason at all,
/// because there is no connection for either to be about. ...
/// It lives beside the preference registry ... it is one rule about the RESOURCE which every write of
/// a preference's configuration — the create and the update — applies whole, so two
/// preferences holding the same configuration cannot report different state because they
/// reached it by different paths.
pub fn apply_plane_verification_statuses(preference: &mut ProcurementPortalPreference) {
```
and what it enforces, state.rs:672-681:
```
    for (enabled, status, reason) in planes {
        if enabled {
            if status.is_none() {
                *status = Some(STATUS_PENDING_VERIFICATION.to_string());
            }
        } else {
            *status = None;
            *reason = None;
        }
    }
```

The two siblings that apply it — src/ops/create_procurement_portal_preference.rs:354-356:
```
        // Each plane this request ENABLES starts at PENDING_VERIFICATION, and a plane it
        // leaves disabled carries no status at all.
        crate::state::apply_plane_verification_statuses(&mut preference);
```
and src/ops/put_procurement_portal_preference.rs:295-302:
```
            // The verification state each plane reports after a configuration write, the
            // same rule the create applies: ... a plane it DISABLES carries no
            // status or reason at all, ...
            crate::state::apply_plane_verification_statuses(preference);
```

The handler that skips it — src/ops/update_procurement_portal_preference_status.rs:349-359 (the whole of its write of the two planes):
```
            // A plane the request states takes the stated status, and its reason becomes
            // the context stated with that update — absent when the request gave none. A
            // plane the request says nothing about is left exactly as it was.
            if let Some((status, reason)) = einvoice {
                preference.einvoice_delivery_preference_status = Some(status);
                preference.einvoice_delivery_preference_status_reason = reason;
            }
            if let Some((status, reason)) = purchase_order {
                preference.purchase_order_retrieval_preference_status = Some(status);
                preference.purchase_order_retrieval_preference_status_reason = reason;
            }
```
`grep -n apply_plane_verification_statuses src/ops/update_procurement_portal_preference_status.rs` matches nothing, and neither `einvoice_delivery_enabled` nor `purchase_order_retrieval_enabled` is read anywhere in that file.

The same file does take the other half of the configuration-write contract — update_procurement_portal_preference_status.rs:361-364:
```
            // Every write of this resource advances its `Version` and stamps
            // `LastUpdateDate` with the instant it changed at; `CreateDate` is not touched.
            preference.version = preference.version.saturating_add(1);
```
which src/state.rs:44-48 describes as what "the calls that SUCCESSFULLY write a preference's configuration — the create, the configuration update and the status update" do.

Test coverage never reaches a disabled plane: tests/all/procurement_portal_preference_status.rs:191-208 builds every fixture with `"EinvoiceDeliveryEnabled": true, "PurchaseOrderRetrievalEnabled": true` ("Create a preference with both planes enabled"), and `grep -n 'disabled|Enabled": false' tests/all/procurement_portal_preference_status.rs` matches only `router.set_request_authentication(false)` at :89 — so no test pins the behaviour this report describes.

## Steps to Reproduce

1. Start the emulator.
2. POST / with `X-Amz-Target: Invoicing.CreateProcurementPortalPreference` and body `{"ProcurementPortalName":"ZIP","BuyerDomain":"NetworkID","BuyerIdentifier":"AN01","SupplierDomain":"NetworkID","SupplierIdentifier":"AN02","EinvoiceDeliveryEnabled":false,"PurchaseOrderRetrievalEnabled":true,"Contacts":[{"Name":"Ops","Email":"ops@example.com"}]}`; note the ARN. `GetProcurementPortalPreference` reports no `EinvoiceDeliveryPreferenceStatus`, as `apply_plane_verification_statuses` requires for a disabled plane.
3. POST / with `X-Amz-Target: Invoicing.UpdateProcurementPortalPreferenceStatus` and body `{"ProcurementPortalPreferenceArn":"<arn>","EinvoiceDeliveryPreferenceStatus":"ACTIVE","EinvoiceDeliveryPreferenceStatusReason":"activated"}`.
4. Observe HTTP 200. POST / with `X-Amz-Target: Invoicing.GetProcurementPortalPreference` for the same ARN: it now reports `EinvoiceDeliveryEnabled: false` together with `EinvoiceDeliveryPreferenceStatus: "ACTIVE"` and the reason — the combination src/state.rs:642-645 says cannot be reported.
5. POST / with `X-Amz-Target: Invoicing.PutProcurementPortalPreference` and body `{"ProcurementPortalPreferenceArn":"<arn>","Contacts":[{"Name":"Ops","Email":"ops@example.com"}],"EinvoiceDeliveryEnabled":false,"PurchaseOrderRetrievalEnabled":true}` — a write that changes nothing about that plane.
6. `GetProcurementPortalPreference` again: `EinvoiceDeliveryPreferenceStatus` and its reason are gone, silently erased by `apply_plane_verification_statuses`, while `Version` has been advanced twice — so the status a 200 persisted is not state any later reader can rely on.
