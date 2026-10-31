Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: d6b005e462d13d7796b585d0a15f1e7892d2fb1e

# A replayed ClientToken short-circuits the Code check, so any code at all is answered 200 and "completes" the validation

_Filed: 2026-10-29T13:32:36Z_
_Operation: VerifyProcurementPortalValidation | Class: validation_

> _Note: this issue was identified against the code base at commit `d6b005e462d13d7796b585d0a15f1e7892d2fb1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`VerifyProcurementPortalValidation` consults the idempotency ledger BEFORE it looks at the submitted `Code`, and the ledger key does not contain the code. The key is `(operation, ProcurementPortalPreferenceArn, ClientToken)` only, so once ONE successful verify has been recorded for a preference under token `T`, every later request naming that same preference and token is answered HTTP 200 with the preference ARN — whatever `Code` it carries, including a code this service never issued, a code of a completely different shape, or the code of another preference.

`Code` is `@required` on this operation, so it is part of the request the token is supposed to identify a repeat of; the handler's replay branch returns before `input.code` is read at all. The result is an invalid input accepted without the refusal the handler itself says it owes: the caller is told the ownership/connectivity validation succeeded on a code that matches nothing outstanding.

This is not the Put-drops-its-ClientToken gap already on the backlog (20261028T175234Z): this handler DOES consult the ledger, and the defect is that the ledger lookup is placed in front of — and is blind to — the one member the operation exists to check.

## Evidence

The handler's OWN stated rule, which the replay path breaks — services/service-com-amazonaws-invoicing-invoicing/src/ops/verify_procurement_portal_validation.rs:268-278:
```
            // A code is accepted only when it is the one this service issued for THIS
            // preference and is still outstanding. Anything else — no send has been
            // made, or a code that does not match the issued one — is refused with the
            // modelled 400, because completing the validation would otherwise assert a
            // connectivity check that never happened.
            let outstanding = stored
                .validation
                .as_ref()
                .filter(|validation| validation.verified_at.is_none())
                .map(|validation| validation.code.as_str());
            if outstanding != Some(input.code.as_str()) {
```

The branch that returns BEFORE that check ever runs, and never reads `input.code` — verify_procurement_portal_validation.rs:242-252:
```
        // `@idempotent`: a replay (the same `ClientToken` against the same preference)
        // answers with the ARN the first call returned; the code was already consumed.
        if let Some(arn) = store.replayed_result(
            Self::NAME,
            &input.procurement_portal_preference_arn,
            client_token,
        ) {
            return Ok(VerifyProcurementPortalValidationOutput {
                procurement_portal_preference_arn: arn,
            });
        }
```

The ledger key that makes the code invisible — services/service-com-amazonaws-invoicing-invoicing/src/state.rs:120-123:
```
    /// The result an `@idempotent` operation already produced for a `ClientToken`,
    /// keyed by `(operation, target, token)`: one token replayed against another
    /// operation, or against another invoice unit, is a different request and must not
    /// replay this one's result. Bounded by [`MAX_IDEMPOTENT_RECORDS`].
    idempotent_results: BTreeMap<IdempotencyKey, String>,
```
and the `target` the handler passes is the preference ARN alone (verify_procurement_portal_validation.rs:324-329 records under the same three parts).

The model makes `Code` part of the request — `smithy_query(query="operation", operation="VerifyProcurementPortalValidation")`: `"Code": {"target": "BasicStringWithoutSpace", "required": true, "length": {"min": 0, "max": 1024}, "pattern": "^\\S+$"}` — so a request with a different `Code` is not a repeat of the recorded one.

Test coverage does NOT pin this: every replay the suite drives re-sends the SAME code, so nothing asserts a 200 for a mismatched code under a replayed token — tests/all/retired_units_and_portal_validation.rs:935-958:
```
    let verified = ok(
        &router,
        "VerifyProcurementPortalValidation",
        &json!({"ProcurementPortalPreferenceArn": first, "Code": issued, "ClientToken": "verify-1"}),
    )
...
    // `@idempotent`: the same token answers the same way without a second code.
    ok(
        &router,
        "VerifyProcurementPortalValidation",
        &json!({"ProcurementPortalPreferenceArn": first, "Code": issued, "ClientToken": "verify-1"}),
    )
```
and the refusal table that pins the 400 for a code nothing issued never carries a `ClientToken` the preference has already seen — tests/all/refused_procurement_portal_writes.rs:250-258.

## Steps to Reproduce

1. Start the emulator.
2. POST / with `X-Amz-Target: Invoicing.CreateProcurementPortalPreference` and body `{"ProcurementPortalName":"ZIP","BuyerDomain":"NetworkID","BuyerIdentifier":"AN01","SupplierDomain":"NetworkID","SupplierIdentifier":"AN02","EinvoiceDeliveryEnabled":true,"PurchaseOrderRetrievalEnabled":true,"Contacts":[{"Name":"Ops","Email":"ops@example.com"}]}`; note the returned `ProcurementPortalPreferenceArn`.
3. POST / with `X-Amz-Target: Invoicing.SendProcurementPortalValidation` and body `{"ProcurementPortalPreferenceArn":"<arn>"}` to issue a code, and read the issued code out of state (the modelled response carries only the ARN).
4. POST / with `X-Amz-Target: Invoicing.VerifyProcurementPortalValidation` and body `{"ProcurementPortalPreferenceArn":"<arn>","Code":"<the issued code>","ClientToken":"v-1"}` — HTTP 200, validation completed.
5. POST / with `X-Amz-Target: Invoicing.VerifyProcurementPortalValidation` and body `{"ProcurementPortalPreferenceArn":"<arn>","Code":"000000-nothing-issued-this","ClientToken":"v-1"}` — the SAME token, a code nothing ever issued.
6. Observe HTTP 200 carrying the preference ARN. Repeat the same request with `ClientToken` omitted and observe HTTP 400 `ValidationException` — the identical code is refused on one path and accepted on the other, although the handler's own comment states that "a code that does not match the issued one — is refused with the modelled 400, because completing the validation would otherwise assert a connectivity check that never happened".
