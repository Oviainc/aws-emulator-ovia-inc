Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d6b005e462d13d7796b585d0a15f1e7892d2fb1e

# A ClientToken reused against the same invoice unit with a DIFFERENT configuration is answered 200 and the write is silently discarded

_Filed: 2026-10-29T13:32:36Z_
_Operation: UpdateInvoiceUnit | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d6b005e462d13d7796b585d0a15f1e7892d2fb1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateInvoiceUnit` looks the idempotency ledger up on `(operation, InvoiceUnitArn, ClientToken)` and returns before it reads any of the members that say what to write (`Description`, `Rule`, `TaxInheritanceDisabled`). So a second update of the SAME unit that carries the same `ClientToken` but a different configuration is treated as a replay: the handler answers HTTP 200 with the unit's ARN and the requested change is never applied, never versioned and never visible to `GetInvoiceUnit`/`ListInvoiceUnits`.

That is a lost write reported as a success, and it contradicts the rule this crate states for the ledger in two places: the ledger is documented as replaying "that same request", and the crate's own test spells out that "A `ClientToken` identifies a repeat of ONE request" whose "write must be applied, not discarded" — a criterion the handler applies only to the unit being named, not to what the request writes. The sibling creates key their replay on content (`CreateInvoiceUnit` on `Name`, `CreateProcurementPortalPreference` on the preference identity), so the two write families disagree about what a token identifies.

`UpdateProcurementPortalPreferenceStatus` has the same content-blind lookup (update_procurement_portal_preference_status.rs:344-352), where it drops a second, different partial status update; the invoice-unit path is the one filed here because it is the simplest to observe.

## Evidence

The ledger's documented contract — services/service-com-amazonaws-invoicing-invoicing/src/state.rs:462-464:
```
    /// Record the ARN `operation` returned for this `target` and `client_token`, so a
    /// replay of that same request answers with this same ARN instead of changing state
    /// a second time.
```
and the field doc that enumerates what makes a request different — state.rs:120-123:
```
    /// The result an `@idempotent` operation already produced for a `ClientToken`,
    /// keyed by `(operation, target, token)`: one token replayed against another
    /// operation, or against another invoice unit, is a different request and must not
    /// replay this one's result. Bounded by [`MAX_IDEMPOTENT_RECORDS`].
```

The crate's own test states the rule in full — services/service-com-amazonaws-invoicing-invoicing/tests/all/invoice_unit_lifecycle.rs:893-895:
```
/// A `ClientToken` identifies a repeat of ONE request, so the same token used against a
/// different invoice unit is a different request: its write must be applied, not
/// discarded, and the response must name the unit the caller asked about.
```

The code under it, which returns before any configuration member is read — services/service-com-amazonaws-invoicing-invoicing/src/ops/update_invoice_unit.rs:169-181:
```
        // `@idempotent`: a replay (the same `ClientToken` against the same invoice
        // unit) answers with the ARN the first call returned and applies the change
        // once. The same token against a DIFFERENT unit is a different request and is
        // applied on its own.
        if let Some(arn) =
            store.replayed_result(Self::NAME, &input.invoice_unit_arn, client_token)
        {
            if store.live_invoice_unit(&arn).is_some() {
                return Ok(UpdateInvoiceUnitOutput {
                    invoice_unit_arn: Some(arn),
                });
            }
        }
```
The members that are consequently never consulted — update_invoice_unit.rs:194-206:
```
            // An omitted member leaves that part of the configuration as it was; a
            // member the request carries replaces it.
            let next = crate::state::InvoiceUnitVersion {
                description: input
                    .description
                    .or_else(|| unit.current.description.clone()),
                tax_inheritance_disabled: input
                    .tax_inheritance_disabled
                    .unwrap_or(unit.current.tax_inheritance_disabled),
                rule: input.rule.unwrap_or_else(|| unit.current.rule.clone()),
                last_modified: crate::state::now(),
            };
```

The sibling writes that DO key on the request's content, so the two families disagree — create_invoice_unit.rs:171 `store.replayed_result(Self::NAME, &input.name, client_token)` and create_procurement_portal_preference.rs:257 `store.replayed_result(Self::NAME, &identity, client_token)` (identity built from the portal and buyer/supplier pair at create_procurement_portal_preference.rs:236-242).

Test coverage: the suite only ever reuses a token across DIFFERENT units (invoice_unit_lifecycle.rs:897-933) or uses a fresh token per update (invoice_unit_lifecycle.rs:1040-1051 `"ClientToken": format!("upd-{revision}")`, :830 `"ClientToken": "upd-1"`). No test asserts the answer for one token reused against one unit with a changed body, so nothing pins today's behaviour.

## Steps to Reproduce

1. Start the emulator.
2. POST / with `X-Amz-Target: Invoicing.CreateInvoiceUnit` and body `{"Name":"alpha","InvoiceReceiver":"111111111111","TaxInheritanceDisabled":false,"Rule":{"LinkedAccounts":["222222222222"]}}`; note the returned `InvoiceUnitArn`.
3. POST / with `X-Amz-Target: Invoicing.UpdateInvoiceUnit` and body `{"InvoiceUnitArn":"<arn>","Description":"first","ClientToken":"tok-1"}` — HTTP 200.
4. POST / with `X-Amz-Target: Invoicing.UpdateInvoiceUnit` and body `{"InvoiceUnitArn":"<arn>","Description":"second","ClientToken":"tok-1"}` — a DIFFERENT request (different `Description`) under the same token.
5. Observe HTTP 200 carrying the ARN.
6. POST / with `X-Amz-Target: Invoicing.GetInvoiceUnit` and body `{"InvoiceUnitArn":"<arn>"}`: `Description` is still `"first"` and `LastModified` is still the first update's instant — the second request's write was discarded while the caller was told it succeeded. Repeat step 4 with `ClientToken` omitted and `"second"` IS written, so the token alone decides whether a stated configuration is applied.
7. Same shape on the portal plane: two `UpdateProcurementPortalPreferenceStatus` calls against one preference under one `ClientToken`, the first stating `EinvoiceDeliveryPreferenceStatus` and the second stating `PurchaseOrderRetrievalPreferenceStatus`, leave the second plane's status unmoved behind a 200.
