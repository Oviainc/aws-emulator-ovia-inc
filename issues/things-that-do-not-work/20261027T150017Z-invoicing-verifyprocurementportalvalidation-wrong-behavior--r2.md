Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# A successful VerifyProcurementPortalValidation leaves both planes at PENDING_VERIFICATION, so no reader can tell the validation completed

_Filed: 2026-10-27T15:00:17Z_
_Operation: VerifyProcurementPortalValidation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate's state module states the transition this operation is supposed to perform: `PENDING_VERIFICATION` is "the one a freshly created preference starts at" and "nothing but a caller's verification (or an explicit `UpdateProcurementPortalPreferenceStatus`) moves it". The handler's success path never moves it.

On a submitted code that matches the outstanding one, the handler writes exactly one field — `validation.verified_at` — which is NOT a modelled member and is reported by nothing: `StoredProcurementPortalPreference::to_summary` and `GetProcurementPortalPreference` both report `preference` only. `einvoice_delivery_preference_status` and `purchase_order_retrieval_preference_status` keep the `PENDING_VERIFICATION` the create gave them, so after a 200 from the call the model documents as the one that "complete[s] the validation of a procurement portal preference", every reader still reports the preference as unverified, and the modelled status value `VALIDATED` is unreachable except by an explicit `UpdateProcurementPortalPreferenceStatus` call.

This is the SUCCESS path only; the refused path correctly leaves everything a reader observes untouched (that is what the handler's doc at lines 13-19 describes, and it is right). The two paths together mean no outcome of this operation ever changes a modelled member, while `crate::state` says a caller's verification is precisely what moves the status.

## Evidence

The rule, in this crate — services/service-com-amazonaws-invoicing-invoicing/src/state.rs:626-634:
```
/// `ProcurementPortalPreferenceStatus$PENDING_VERIFICATION` — the status a preference
/// plane holds while the portal connection has not been verified yet. One of the values
/// the modelled enum admits
/// ([`crate::ops::types::PROCUREMENT_PORTAL_PREFERENCE_STATUS_VALUES`]), and the one a
/// freshly created preference starts at: the service models
/// `SendProcurementPortalValidation` and `VerifyProcurementPortalValidation` as the
/// calls that carry a preference past it, so nothing but a caller's verification (or an
/// explicit `UpdateProcurementPortalPreferenceStatus`) moves it.
pub const STATUS_PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";
```

The code under it — services/service-com-amazonaws-invoicing-invoicing/src/ops/verify_procurement_portal_validation.rs:300-307 (the whole of the success path's write):
```
            // Submitting the issued code is what completes the validation, so this call
            // writes: the instant it was completed at is recorded against the
            // preference, which also retires the code — a second submit of the same
            // code is no longer outstanding.
            let verified_at = crate::state::now();
            if let Some(validation) = stored.validation.as_mut() {
                validation.verified_at = Some(verified_at);
            }
```
— no assignment to `preference.einvoice_delivery_preference_status` or `preference.purchase_order_retrieval_preference_status` exists anywhere in this file.

What the create left for it to move — src/ops/create_procurement_portal_preference.rs:308-314, 347-351:
```
        // Each plane starts PENDING_VERIFICATION, and only a plane this request ENABLES
        // has a verification status at all: ... the service models
        // `SendProcurementPortalValidation`/`VerifyProcurementPortalValidation` as the
        // calls that carry it past that status.
        let pending =
            |enabled: bool| enabled.then(|| crate::state::STATUS_PENDING_VERIFICATION.to_string());
...
            einvoice_delivery_preference_status: pending(input.einvoice_delivery_enabled),
```

That the written field is invisible to every reader — src/state.rs:564-571 (`IssuedPortalValidation` is held beside `preference`, not inside it: "Kept in state and never put on the wire") and src/ops/get_procurement_portal_preference.rs:180-182, which answers `procurement_portal_preference: stored.preference.clone()` — `validation` is not part of it.

Model, `smithy_query(query="operation", operation="VerifyProcurementPortalValidation")` documentation: "Submits a validation code to complete the validation of a procurement portal preference. Use this operation after calling SendProcurementPortalValidation to confirm ownership and connectivity of the configured procurement portal endpoint." — and `ProcurementPortalPreferenceStatus` admits `VALIDATED` (`smithy_query(query="shape", shape="ProcurementPortalPreferenceStatus")`).

Test coverage asserts only the invisible field, never the reported status — tests/all/retired_units_and_portal_validation.rs:939-955:
```
    let verified = ok(
        &router,
        "VerifyProcurementPortalValidation",
        &json!({"ProcurementPortalPreferenceArn": first, "Code": issued, "ClientToken": "verify-1"}),
    )
    .await;
...
    assert!(
        is_verified(&engine, &first),
        "case verify with the issued code: the validation is recorded as completed"
    );
```
where `is_verified` (:770-778) reads `stored.validation...verified_at` straight out of state; no assertion in tests/all/ reads a status after a successful verify (`grep -n VALIDATED tests/` matches only procurement_portal_preference_status.rs:40,609-610, which drive `UpdateProcurementPortalPreferenceStatus`).

## Steps to Reproduce

1. Start the emulator and create a preference with `EinvoiceDeliveryEnabled: true` (`CreateProcurementPortalPreference`); `GetProcurementPortalPreference` reports `EinvoiceDeliveryPreferenceStatus: "PENDING_VERIFICATION"`.
2. Make a code outstanding for that ARN the way the suite does (tests/all/retired_units_and_portal_validation.rs:739-754 `issue_code`: `store.issue_portal_validation_code()` then set `stored.validation = Some(IssuedPortalValidation { code, issued_at, verified_at: None })`). `SendProcurementPortalValidation` cannot issue one over the wire — src/ops/send_procurement_portal_validation.rs:263-270 always answers the modelled 400.
3. POST / with `X-Amz-Target: Invoicing.VerifyProcurementPortalValidation` and body `{"ProcurementPortalPreferenceArn":"<arn>","Code":"<the issued code>"}`; observe HTTP 200 with the ARN.
4. POST / with `X-Amz-Target: Invoicing.GetProcurementPortalPreference` for the same ARN: `EinvoiceDeliveryPreferenceStatus` is still `"PENDING_VERIFICATION"`, and `ListProcurementPortalPreferences` reports the same — no modelled member moved, although src/state.rs:630-633 states that a caller's verification is what carries a preference past that status.
