Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a67556f919df77bd586efc9ebe2e6b88c4f9f9e7

# A write that leaves Marketplace PunchOut disabled still stores its MarketplacePunchOutPreference, so a reader reports a configuration the model says is present only when the feature is enabled

_Filed: 2026-10-28T19:51:19Z_
_Operation: CreateProcurementPortalPreference | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a67556f919df77bd586efc9ebe2e6b88c4f9f9e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both writes of a procurement portal preference store `MarketplacePunchOutPreference` unconditionally, independently of the `MarketplacePunchOutEnabled` value they resolve.

`crate::state::resolve_marketplace_punch_out_enabled` turns an omitted `MarketplacePunchOutEnabled` into `false` (state.rs:770). The create then stores that resolved `false` in `marketplace_punch_out_enabled` and, on the very next line, stores the caller's `MarketplacePunchOutPreference` verbatim in `marketplace_punch_out_preference` — no branch ties the second to the first. So a create that states a `MarketplacePunchOutPreference` but omits (or sets `false`) `MarketplacePunchOutEnabled` is answered 200 and `GetProcurementPortalPreference` / `ListProcurementPortalPreferences` then report a preference holding `MarketplacePunchOutEnabled: false` TOGETHER WITH a `MarketplacePunchOutPreference` — the combination the stored shape's own documentation excludes ("This is present when `MarketplacePunchOutEnabled` is `true`").

`PutProcurementPortalPreference` has the same hole from the other side: it writes the resolved flag (line 255) but only replaces the configuration when the request states one (lines 271-273), so a Put that disables Marketplace PunchOut leaves the configuration an earlier write stored sitting beside the now-`false` flag, and no request can ever take it off again.

This is the rule the SAME write path already applies to the two verification planes and only to them: `crate::state::apply_plane_verification_statuses` — called by the create at line 356 and by the Put at line 302 — clears a disabled plane's status AND its reason, documented as being so "because there is no connection for either to be about". The Marketplace PunchOut flag/configuration pair is the identical shape of dependent member and is left untouched, so one dependent pair is normalised on disable and the other is not.

No test pins the current behaviour: tests/all/shared_marketplace_punch_out_rule.rs drives every (portal x flag x configuration) combination through both writes but asserts only the stored FLAG (`reported_punch_out_enabled`), never the presence or absence of `MarketplacePunchOutPreference`.

## Evidence

Smithy, `smithy_query(query="shape", shape="ProcurementPortalPreference")`, member `MarketplacePunchOutPreference`:
  "The Marketplace PunchOut configuration for this procurement portal preference. This is present when <code>MarketplacePunchOutEnabled</code> is <code>true</code>."

The resolved flag — services/service-com-amazonaws-invoicing-invoicing/src/state.rs:770:
```
    let enabled = marketplace_punch_out_enabled.unwrap_or(false);
```

The create storing both with no link between them — src/ops/create_procurement_portal_preference.rs:337-338:
```
            marketplace_punch_out_enabled: Some(marketplace_punch_out_enabled),
            marketplace_punch_out_preference: input.marketplace_punch_out_preference,
```

The Put, same hole from the other side — src/ops/put_procurement_portal_preference.rs:255 and :271-273:
```
            preference.marketplace_punch_out_enabled = Some(marketplace_punch_out_enabled);
```
```
            if let Some(punch_out) = input.marketplace_punch_out_preference {
                preference.marketplace_punch_out_preference = Some(punch_out);
            }
```

The rule this crate already states for a DISABLED feature's dependent members — src/state.rs:644-647 (doc of `apply_plane_verification_statuses`, which the create calls at create_procurement_portal_preference.rs:356 and the Put at put_procurement_portal_preference.rs:302):
```
/// pair describes the portal connection an ENABLED plane has: a plane the configuration
/// enables and nothing has verified yet reports [`STATUS_PENDING_VERIFICATION`], and a
/// plane the configuration leaves DISABLED reports no status and no reason at all,
/// because there is no connection for either to be about.
```
and the code under it — src/state.rs:677-686:
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

Test coverage: tests/all/shared_marketplace_punch_out_rule.rs:199-230 enumerates `enabled` in `[Some(true), Some(false), None]` x `config` in `[false, true]` for all four portals, and the only stored value it asserts is the flag — :162-174:
```
/// The `MarketplacePunchOutEnabled` `GetProcurementPortalPreference` reports for `arn`.
async fn reported_punch_out_enabled(router: &MultiServiceRouter, arn: &str) -> Option<bool> {
```
— so the `enabled: Some(false), config: true` rows run today and assert nothing about the configuration that was stored beside the false flag.

## Steps to Reproduce

1. Start the emulator.
2. POST / with `X-Amz-Target: Invoicing.CreateProcurementPortalPreference` and body `{"ProcurementPortalName":"ZIP","BuyerDomain":"NetworkID","BuyerIdentifier":"AN01","SupplierDomain":"NetworkID","SupplierIdentifier":"AN02","EinvoiceDeliveryEnabled":true,"PurchaseOrderRetrievalEnabled":true,"Contacts":[{"Name":"Ops","Email":"ops@example.com"}],"MarketplacePunchOutPreference":{"ApprovalRequestRedirectUrl":"https://example.com/approve"}}` — `MarketplacePunchOutEnabled` omitted, so it resolves to `false`.
3. Observe HTTP 200 and note the returned `ProcurementPortalPreferenceArn`.
4. POST / with `X-Amz-Target: Invoicing.GetProcurementPortalPreference` and body `{"ProcurementPortalPreferenceArn":"<arn>"}`.
5. Observe the reported preference carries `MarketplacePunchOutEnabled: false` AND a `MarketplacePunchOutPreference` — the model documents that member as present when the flag is true, and the same write path clears a disabled verification plane's status and reason for exactly this reason.
6. Second path: create a preference with `MarketplacePunchOutEnabled: true` plus a `MarketplacePunchOutPreference`, then POST `Invoicing.PutProcurementPortalPreference` with `{"ProcurementPortalPreferenceArn":"<arn>","Contacts":[{"Name":"Ops","Email":"ops@example.com"}],"EinvoiceDeliveryEnabled":true,"PurchaseOrderRetrievalEnabled":true,"MarketplacePunchOutEnabled":false}`. Read the preference back: the flag is now `false` and the earlier `MarketplacePunchOutPreference` is still reported, with no request able to remove it.
