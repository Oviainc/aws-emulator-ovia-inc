Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f84dda0f3deaf97a2c7ee51f672a37b240346b3

# SendProcurementPortalValidation never issues a validation code and refuses every well-formed request

_Filed: 2026-10-28T03:53:28Z_
_Operation: SendProcurementPortalValidation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f84dda0f3deaf97a2c7ee51f672a37b240346b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents this operation as the one that ISSUES the validation code the flow runs on. The handler issues nothing: after resolving the ARN it unconditionally returns the modelled `ValidationException` (HTTP 400), so no well-formed request against an existing preference can ever succeed and the operation's modelled 200 output (`ProcurementPortalPreferenceArn`, `@required`) is unreachable.

The consequence is that the whole portal-validation flow is dead over the wire. `StoredProcurementPortalPreference::validation` — the field that holds the outstanding code — is assigned exactly once in the crate, to `None`, by `CreateProcurementPortalPreference`; no handler ever sets it to `Some(..)`. `VerifyProcurementPortalValidation`'s success branch therefore cannot be reached by any sequence of API calls: `outstanding` is always `None`, so every submitted `Code` falls into the refusal branch. The crate's own test suite has to reach into state and set `validation` by hand to exercise the verify at all.

The gap is admitted in the crate: `ComAmazonawsInvoicingInvoicingState::issue_portal_validation_code` exists, mints exactly the six-digit code the flow needs, says "No operation of this emulator issues one today", and has no caller in `src/` — it is dead code reachable only from tests. Two other doc comments in the same module assert the opposite of what the handler does ("The validation `SendProcurementPortalValidation` last issued for this preference", "The code `SendProcurementPortalValidation` issued").

The refusal also mis-states the cause to the client: `reason: "other"` with a message about an unconfirmed delivery, for a request that satisfies every modelled constraint.

## Evidence

Smithy — `smithy_query(query="doc", shape="SendProcurementPortalValidation")`:
  "Sends a validation request for a procurement portal preference. This operation initiates the validation process by **issuing a validation code** that confirms ownership and connectivity of the configured procurement portal endpoint. Use `VerifyProcurementPortalValidation` to submit the received code and complete validation."

The handler does the opposite — services/service-com-amazonaws-invoicing-invoicing/src/ops/send_procurement_portal_validation.rs:253-270:
```
        // A code exists only once the test e-invoice carrying it has been delivered to
        // the preference's procurement portal. ... so the
        // delivery is never confirmed and the call is refused with the modelled 400 —
        // it issues no code, so no `VerifyProcurementPortalValidation` can complete a
        // connection nothing tested.
        ...
        Err(
            SendProcurementPortalValidationErrorModeled::ValidationException(
                test_einvoice_delivery_not_confirmed(test_einvoice_delivery_endpoint(
                    &stored.preference,
                )),
            )
            .into(),
        )
```
There is no other `return`/`Ok` in this `handle` body (the only earlier exit is the 404 at :245-251), so every request is a 400.

The comment that admits the gap, on the unused minting helper — src/state.rs:419-433:
```
    /// A procurement-portal validation code: six decimal digits, the form the
    /// portal-validation flow's `Code` is quoted in.
    ...
    /// No operation of this emulator issues one today:
    /// `SendProcurementPortalValidation` cannot confirm that delivery (see that
    /// operation's module docs) and so answers the modelled 400 instead of minting a
    /// code. This is where a code comes from for whatever can confirm it.
    pub fn issue_portal_validation_code(&mut self) -> String {
```
`grep -n issue_portal_validation_code services/service-com-amazonaws-invoicing-invoicing` matches only state.rs:433 (the definition) and two TEST call sites (tests/all/retired_units_and_portal_validation.rs:742, tests/all/refused_procurement_portal_writes.rs:655) — no handler calls it.

State doc asserting what the handler never does — src/state.rs:546-549 and :565-568:
```
    /// The validation `SendProcurementPortalValidation` last issued for this
    /// preference, if any. `None` means no code is outstanding, so there is
    /// no code a `VerifyProcurementPortalValidation` could match.
    pub validation: Option<IssuedPortalValidation>,
```
```
    /// The code `SendProcurementPortalValidation` issued. Kept in state and never put
    /// on the wire: ...
    pub code: String,
```
The only assignment of that field in `src/` — src/ops/create_procurement_portal_preference.rs:361-363:
```
                // Nothing has requested a portal validation for this preference yet, so
                // no code is outstanding and no submission has been refused.
                validation: None,
```
so the verify's accept branch (src/ops/verify_procurement_portal_validation.rs:265-270 `let outstanding = stored.validation.as_ref()...; if outstanding != Some(input.code.as_str())`) can never be satisfied through the API.

Test coverage pins the 400 and works around it instead of driving it — tests/all/retired_units_and_portal_validation.rs:730-753:
```
/// Make a validation code outstanding for `arn`, as a CONFIRMED delivery of the test
/// e-invoice would.
///
/// `SendProcurementPortalValidation` cannot confirm that delivery in this emulator — it
/// performs no outbound delivery and hosts no procurement portal, so the send answers the
/// modelled 400 ...
fn issue_code(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> String {
    ... store.issue_portal_validation_code(); ... .validation = Some(IssuedPortalValidation { ... });
```
and :863-874:
```
        assert_eq!(
            status, 400,
            "case send {arn}: the send cannot confirm delivery, so it refuses: got {status} {body}"
        );
```
— the stated reason for that 400 is contradicted by the operation's own modelled documentation above, which makes issuing the code the operation's job rather than a consequence of a confirmed delivery.

## Steps to Reproduce

1. Start the emulator.
2. POST / with `X-Amz-Target: Invoicing.CreateProcurementPortalPreference` and body `{"ProcurementPortalName":"ZIP","BuyerDomain":"NetworkID","BuyerIdentifier":"AN01","SupplierDomain":"NetworkID","SupplierIdentifier":"AN02","EinvoiceDeliveryEnabled":true,"PurchaseOrderRetrievalEnabled":true,"ProcurementPortalInstanceEndpoint":"https://portal.example.com/cxml","Contacts":[{"Name":"Ops","Email":"ops@example.com"}]}`; note the returned `ProcurementPortalPreferenceArn`.
3. POST / with `X-Amz-Target: Invoicing.SendProcurementPortalValidation` and body `{"ProcurementPortalPreferenceArn":"<arn>"}` — an input that satisfies every modelled constraint and names a preference that exists.
4. Observe HTTP 400 `ValidationException` (`reason: "other"`). The model documents the operation as "initiates the validation process by issuing a validation code", so the expected answer is HTTP 200 with the ARN and an outstanding code recorded against the preference.
5. POST / with `X-Amz-Target: Invoicing.VerifyProcurementPortalValidation` and body `{"ProcurementPortalPreferenceArn":"<arn>","Code":"123456"}` — and with any other code: every submission is refused, because `stored.validation` is still `None` and no API call can ever make it `Some`.
