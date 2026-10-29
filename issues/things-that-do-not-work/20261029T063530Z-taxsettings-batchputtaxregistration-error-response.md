Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 6c829b3943454ada5df5dfe2630a7a4c6694ce1c

# A per-account conflict row reports code "ConflictException" — the exception shape name this crate states must never go in that member

_Filed: 2026-10-29T06:35:30Z_
_Operation: BatchPutTaxRegistration | Class: error-response_

> _Note: this issue was identified against the code base at commit `6c829b3943454ada5df5dfe2630a7a4c6694ce1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BatchPutTaxRegistrationError$code` targets the model's free-form `ErrorCode` shape and is OPTIONAL (smithy_query(query="shape", shape="BatchPutTaxRegistrationError") shows no `required` on it). This crate states one rule for that shape in `ops/types/shared_types.rs`: it carries a SERVICE-DEFINED code and never the exception's own shape name, and where this service publishes no code the member is left OFF the wire rather than filled with the shape name — which is exactly why `ConflictException`, `InternalServerException` and `ResourceNotFoundException` are declared `unsupported = ["errorCode"]`.

The per-account rows of this handler follow that rule for a validation refusal (`code: Some(refusal.error_code)`, e.g. `GSTExistenceBlockSetPAN`) and break it for a conflict refusal: the row is filled with the literal string `"ConflictException"`. So one client reading `errors[].code` gets a service discriminator from one branch of the same loop and an exception shape name from the other — the one value shared_types.rs says a client would mistake for a service code.

The crate's own remedy is available here and needs no new value: the member is optional, so a conflict row can omit `code` exactly as the 409 response omits `errorCode`. The crate's test file even asserts both conventions a few lines apart (see evidence), so the inconsistency is pinned on both sides.

Not the already-filed `BatchDeleteTaxRegistration` error-response issue: that one is about `check_registration_deletable`'s discarded `FieldValidationFailed` code in a different handler, and it explicitly states "BatchPutTaxRegistration follows that rule" — true only of this loop's validation branch, not of its conflict branch.

## Evidence

The crate-wide rule for this member — `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/types/shared_types.rs:14-27`:
```
// The model marks each one's `errorCode` `@required`, but it targets `ErrorCode` — a
// free-form string (length 0..50), NOT the `ValidationExceptionErrorCode` enum — and it
// carries a SERVICE-DEFINED code, never the exception's own shape name, which the wire
// already carries in `x-amzn-errortype` / `__type`. This service publishes no code for a
// missing resource, a state conflict or an internal fault, ... so the three are declared unsupported and left
// off the wire instead of being filled with the shape name — which a client reading
// `errorCode` to classify a failure would take for a service code.
```

The code under it — `src/ops/batch_put_tax_registration.rs:251-269`, the two branches of one loop:
```
            if let Err(refusal) = store.check_registration_type_against_stored(account_id, entry) {
                errors.push(BatchPutTaxRegistrationError2 {
                    account_id: account_id.to_string(),
                    message: refusal.message,
                    code: Some(refusal.error_code),
                });
                continue;
            }
            match store.store_registration(account_id, entry, written_at) {
                Ok(status) => stored_status = Some(status),
                Err(conflict) => errors.push(BatchPutTaxRegistrationError2 {
                    account_id: account_id.to_string(),
                    message: conflict.message,
                    // The code this account's own single-account write would
                    // have failed with: `PutTaxRegistration` answers both
                    // refusals with its modelled `ConflictException`.
                    code: Some("ConflictException".to_string()),
                }),
            }
```

One test file asserts both conventions — `tests/all/pending_trn_conflict.rs:233-236`:
```
        assert_eq!(
            rows[0].1, "ConflictException",
            "case {case:?}: the row must carry the code the write failed with, got {rows:?}"
        );
```
and `tests/all/pending_trn_conflict.rs:261-268`, on the single-account path of the SAME refusal:
```
    // `errorCode` on this service's exceptions is a service-defined code, never
    // the exception's shape name, and this service publishes none for a
    // conflict — the same body the resident-bound conflict carries.
    assert!(
        answer.body.get("errorCode").is_none(),
        "case {case:?}: the conflict body must not carry an errorCode, got {:?}",
        answer.body
    );
```
Spec note: `BatchPutTaxRegistrationError$code` is `{"target": "ErrorCode", "length": {"min": 0, "max": 50}, "pattern": "^[\\s\\S]*$"}` with no `required`, so omitting it is wire-legal — the same remedy shared_types.rs applies to the exceptions. `tests/all/batch_tax_registration.rs:730` pins the current string as well.

## Steps to Reproduce

1. POST /PutTaxRegistration with {"accountId":"000000000001","taxRegistrationEntry":{"registrationId":"DE136695976","registrationType":"VAT","legalName":"Acme GmbH","legalAddress":{"postalCode":"10115","countryCode":"DE"}}} and do NOT wait for the TRN validation to fall due, so the record stays Pending.
2. POST /BatchPutTaxRegistration with {"accountIds":["000000000001"],"taxRegistrationEntry":{...the same entry...}} — the shared write refuses that account with the documented pending-TRN ConflictException.
3. Read errors[0].code: it is the literal "ConflictException", the exception's own shape name, while the single-account 409 for the identical refusal carries no errorCode at all (shared_types.rs:14-27, pinned by pending_trn_conflict.rs:261-268).
4. For contrast, in the same loop a type-blocked account (stored GST + submitted PAN) is reported with code "GSTExistenceBlockSetPAN", a real service code.
