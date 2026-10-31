Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# PutProcurementPortalPreference accepts Marketplace PunchOut configurations the model documents as invalid

_Filed: 2026-10-27T15:00:17Z_
_Operation: PutProcurementPortalPreference | Class: validation_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The configuration update applies the Marketplace PunchOut precondition in a narrower form than its own modelled contract states, and ignores the second half of that contract entirely.

Symptom 1 — the requirement is gated on the portal. `PutProcurementPortalPreference`'s own input member doc states the requirement WITHOUT any portal qualification: `MarketplacePunchOutPreference` is "Required when MarketplacePunchOutEnabled is true". The handler reaches the shared rule `crate::state::resolve_marketplace_punch_out_enabled`, which refuses only when the stored preference's portal is `COUPA`. So a Put against a preference whose portal is `ZIP`, `SAP_BUSINESS_NETWORK` or `JAGGAER` that states `MarketplacePunchOutEnabled: true` and no `MarketplacePunchOutPreference` is answered 200 and persists `marketplace_punch_out_enabled = true` with no configuration behind it — which the stored shape's own doc also excludes (`ProcurementPortalPreference$MarketplacePunchOutPreference`: "This is present when MarketplacePunchOutEnabled is true"), so a later `GetProcurementPortalPreference` reports a preference the model says cannot exist.

Note for a fixer: `CreateProcurementPortalPreferenceRequest$MarketplacePunchOutPreference` is worded NARROWER ("Required for Coupa when MarketplacePunchOutEnabled is true"), so the create's Coupa-only scope is correct as written. The one home for the rule should keep that scope for the create and take the Put's unqualified scope for the Put, rather than being collapsed back into one portal test or duplicated per handler.

Symptom 2 — the host rule is never applied, on either write. `MarketplacePunchOutPreference$ApprovalRequestRedirectUrl` is documented as: "When provided together with the procurement portal instance endpoint, its host must match the host of that endpoint." Neither the Put nor the create compares the two values: the redirect URL is stored verbatim beside `ProcurementPortalInstanceEndpoint`, and `grep -i host` over this crate's `src/` matches nothing but prose about the emulator hosting no portal. A write naming `https://portal.example.com/approvals` together with an instance endpoint of `https://other.example.net/cxml` is accepted with 200.

## Evidence

Smithy, `smithy_query(query="doc", shape="PutProcurementPortalPreferenceRequest", member="MarketplacePunchOutPreference")`:
  "Configuration for Marketplace PunchOut. Required when MarketplacePunchOutEnabled is true."

The same sentence restated in the handler's own module doc — services/service-com-amazonaws-invoicing-invoicing/src/ops/put_procurement_portal_preference.rs:76-77:
```
//! * `MarketplacePunchOutPreference` (MarketplacePunchOutPreference) — Configuration for
//!   Marketplace PunchOut. Required when MarketplacePunchOutEnabled is true.
```

The code under it — put_procurement_portal_preference.rs:235-241:
```
            let marketplace_punch_out_enabled =
                crate::state::resolve_marketplace_punch_out_enabled(
                    &stored.preference.procurement_portal_name,
                    input.marketplace_punch_out_enabled,
                    input.marketplace_punch_out_preference.as_ref(),
                )
                .map_err(PutProcurementPortalPreferenceErrorModeled::ValidationException)?;
```
and the rule it reaches — services/service-com-amazonaws-invoicing-invoicing/src/state.rs:674-690:
```
    let enabled = marketplace_punch_out_enabled.unwrap_or(false);
    if enabled
        && procurement_portal_name == PORTAL_COUPA
        && marketplace_punch_out_preference.is_none()
    {
        return Err(ValidationException {
```
— the portal test is what makes a non-Coupa Put with `MarketplacePunchOutEnabled: true` and no preference succeed. Nothing else in `state.rs` or in the Put handler refuses that combination.

The narrower create wording this scope came from — `smithy_query(query="doc", shape="CreateProcurementPortalPreferenceRequest", member="MarketplacePunchOutPreference")`: "Required for Coupa when MarketplacePunchOutEnabled is true."

What the stored resource then contradicts — `smithy_query(query="shape", shape="ProcurementPortalPreference")`, member `MarketplacePunchOutPreference`: "The Marketplace PunchOut configuration for this procurement portal preference. This is present when MarketplacePunchOutEnabled is true."

Symptom 2, the unenforced host rule — `smithy_query(query="doc", shape="MarketplacePunchOutPreference", member="ApprovalRequestRedirectUrl")`:
  "The URL that buyers are redirected to for approval requests in the procurement portal. This is only supported for Coupa. When provided together with the procurement portal instance endpoint, its host must match the host of that endpoint."
The Put stores both values without comparing them — put_procurement_portal_preference.rs:260-268:
```
            if let Some(endpoint) = input.procurement_portal_instance_endpoint {
                preference.procurement_portal_instance_endpoint = Some(endpoint);
            }
...
            if let Some(punch_out) = input.marketplace_punch_out_preference {
                preference.marketplace_punch_out_preference = Some(punch_out);
            }
```
and `resolve_marketplace_punch_out_enabled` (state.rs:669-692) is handed no endpoint at all, so no code path can make the comparison.

Test coverage pins the narrower rule for BOTH writes, under a reason quoted from the CREATE's doc — tests/all/shared_marketplace_punch_out_rule.rs:38-41:
```
/// The portal whose Marketplace PunchOut configuration the model documents as required
/// (`MarketplacePunchOutPreference`: "Required for Coupa when MarketplacePunchOutEnabled
/// is true").
const PORTAL_REQUIRING_PUNCH_OUT_CONFIG: &str = "COUPA";
```
and :219-223:
```
    fn refused(&self) -> bool {
        self.enabled == Some(true)
            && self.portal == PORTAL_REQUIRING_PUNCH_OUT_CONFIG
            && !self.config
    }
```
— the Put rows of that table therefore assert a 200 for every non-Coupa portal, which is the behaviour the Put's own member doc refuses. No test anywhere in tests/all/ asserts anything about the redirect URL's host (the only occurrences are fixture values: procurement_portal_preference_reads.rs:276, shared_marketplace_punch_out_rule.rs:114, procurement_portal_preference_lifecycle.rs:293,422, procurement_portal_preferences.rs:240,753).

## Steps to Reproduce

1. Start the emulator.
2. POST / with `X-Amz-Target: Invoicing.CreateProcurementPortalPreference` and body `{"ProcurementPortalName":"ZIP","BuyerDomain":"NetworkID","BuyerIdentifier":"AN01","SupplierDomain":"NetworkID","SupplierIdentifier":"AN02","EinvoiceDeliveryEnabled":true,"PurchaseOrderRetrievalEnabled":true,"Contacts":[{"Name":"Ops","Email":"ops@example.com"}]}`; note the returned `ProcurementPortalPreferenceArn`.
3. POST / with `X-Amz-Target: Invoicing.PutProcurementPortalPreference` and body `{"ProcurementPortalPreferenceArn":"<arn>","Contacts":[{"Name":"Ops","Email":"ops@example.com"}],"EinvoiceDeliveryEnabled":true,"PurchaseOrderRetrievalEnabled":true,"MarketplacePunchOutEnabled":true}` — `MarketplacePunchOutPreference` omitted.
4. Observe HTTP 200. The operation's own member doc says `MarketplacePunchOutPreference` is "Required when MarketplacePunchOutEnabled is true", so this is the modelled `ValidationException` (400).
5. POST / with `X-Amz-Target: Invoicing.GetProcurementPortalPreference` and body `{"ProcurementPortalPreferenceArn":"<arn>"}`: the preference reports `MarketplacePunchOutEnabled: true` with no `MarketplacePunchOutPreference`, the combination the stored shape's doc says cannot occur.
6. Second symptom: repeat the Put with `{"...":"...","MarketplacePunchOutEnabled":true,"MarketplacePunchOutPreference":{"ApprovalRequestRedirectUrl":"https://other.example.net/approve"},"ProcurementPortalInstanceEndpoint":"https://portal.example.com/cxml"}` against a COUPA preference: observe HTTP 200 although the redirect URL's host does not match the instance endpoint's host, which the member doc requires.
