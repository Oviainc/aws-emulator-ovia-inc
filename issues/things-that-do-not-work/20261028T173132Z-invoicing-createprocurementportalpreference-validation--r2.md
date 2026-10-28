Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# CreateProcurementPortalPreference stores any non-space string as ProcurementPortalInstanceEndpoint, though the model documents it must be a valid HTTPS URL

_Filed: 2026-10-28T17:31:32Z_
_Operation: CreateProcurementPortalPreference | Class: validation_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateProcurementPortalPreferenceRequest$ProcurementPortalInstanceEndpoint` is documented as "The endpoint URL where e-invoices will be delivered to the procurement portal. Must be a valid HTTPS URL." The member's only modelled constraints are `BasicStringWithoutSpace`'s `@length(0..1024)` and `@pattern ^\S+$`, so the boundary validator admits `ftp://portal.example.com`, `portal.example.com`, `not-a-url` or `http://portal.example.com` alike. The handler then persists whatever arrived verbatim, with no check of its own, and `GetProcurementPortalPreference` / `ListProcurementPortalPreferences` report it back — so a preference exists whose delivery endpoint the model says cannot be stored, and the modelled `ValidationException` (HTTP 400) the operation declares is never raised for it.

This is not a rule the crate declines to express: the create handler already refuses a model-documented combination the boundary cannot express, through `crate::state::resolve_marketplace_punch_out_enabled`, and nothing in the crate (no constant, comment or test) mentions the HTTPS requirement — `grep -i https src/` matches only the two scaffold doc lines that restate it. So the divergence is unacknowledged rather than deliberate.

Distinct from the fixed issue on this pair (`CreateProcurementPortalPreference` / validation, 20261024T004801Z, the per-resource tag quota) and from the open `PutProcurementPortalPreference` / validation issue (20261027T150017Z), which covers the Marketplace-PunchOut portal scope and the `ApprovalRequestRedirectUrl` host-match rule, not this member's HTTPS requirement.

## Evidence

Smithy, `smithy_query(query="doc", shape="CreateProcurementPortalPreferenceRequest", member="ProcurementPortalInstanceEndpoint")`:
  "The endpoint URL where e-invoices will be delivered to the procurement portal. Must be a valid HTTPS URL."

The same sentence restated in the handler's own module doc — services/service-com-amazonaws-invoicing-invoicing/src/ops/create_procurement_portal_preference.rs:74-76:
```
//! * `ProcurementPortalInstanceEndpoint` (BasicStringWithoutSpace, len 0..1024, pattern
//!   ^\S+$) — The endpoint URL where e-invoices will be delivered to the procurement portal.
//!   Must be a valid HTTPS URL.
```

The code under it stores the value with no test of any kind — create_procurement_portal_preference.rs:317-318:
```
            procurement_portal_shared_secret: input.procurement_portal_shared_secret,
            procurement_portal_instance_endpoint: input.procurement_portal_instance_endpoint,
```

The only refusals this handler raises before that write are the Marketplace-PunchOut combination and the tag quota — create_procurement_portal_preference.rs:210-215:
```
        let marketplace_punch_out_enabled = crate::state::resolve_marketplace_punch_out_enabled(
            &input.procurement_portal_name,
            input.marketplace_punch_out_enabled,
            input.marketplace_punch_out_preference.as_ref(),
        )
        .map_err(CreateProcurementPortalPreferenceErrorModeled::ValidationException)?;
```
— which is the precedent that handler-side refusal of a model-documented rule the boundary cannot express is how this crate states such rules (state.rs:751-753: "a combination of members the model cannot express, so the boundary validator passes it and it is refused here as the modelled `ValidationException`").

The member's modelled constraints, which admit any non-space string — `smithy_query(query="operation", operation="CreateProcurementPortalPreference")`: `"ProcurementPortalInstanceEndpoint": {"target": "BasicStringWithoutSpace", "length": {"min": 0, "max": 1024}, "pattern": "^\\S+$"}`.

No crate code mentions the rule: `grep -ni 'https' services/service-com-amazonaws-invoicing-invoicing/src/` returns exactly two hits, both the scaffold doc line above and its twin in put_procurement_portal_preference.rs:85.

Test coverage: every test that supplies this member supplies an `https://` value, so none pins the acceptance of a non-HTTPS endpoint and none would have to change its assertions — tests/all/procurement_portal_preferences.rs:247, tests/all/procurement_portal_preference_reads.rs:263, tests/all/procurement_portal_preference_lifecycle.rs:236,405,691-702, tests/all/procurement_portal_error_message_key.rs:135.

## Steps to Reproduce

1. Start the emulator.
2. POST / with `X-Amz-Target: Invoicing.CreateProcurementPortalPreference` and body `{"ProcurementPortalName":"ZIP","BuyerDomain":"NetworkID","BuyerIdentifier":"AN01","SupplierDomain":"NetworkID","SupplierIdentifier":"AN02","EinvoiceDeliveryEnabled":true,"PurchaseOrderRetrievalEnabled":true,"Contacts":[{"Name":"Ops","Email":"ops@example.com"}],"ProcurementPortalInstanceEndpoint":"ftp://portal.example.com/cxml"}`.
3. Observe HTTP 200 and a minted `ProcurementPortalPreferenceArn`. The member's own doc says the value "Must be a valid HTTPS URL", so this is the modelled `ValidationException` (HTTP 400) the operation declares.
4. POST / with `X-Amz-Target: Invoicing.GetProcurementPortalPreference` and body `{"ProcurementPortalPreferenceArn":"<arn>"}`: the stored preference reports `"ProcurementPortalInstanceEndpoint":"ftp://portal.example.com/cxml"`.
5. Repeat step 2 with `"ProcurementPortalInstanceEndpoint":"not-a-url"` and with `"http://portal.example.com/cxml"`: both are answered 200 and persisted the same way.
