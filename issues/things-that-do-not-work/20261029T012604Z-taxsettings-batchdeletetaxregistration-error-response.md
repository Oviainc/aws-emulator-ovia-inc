Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 0b0649dc1b525c86072f831fd9c0b574c3cd2f61

# Per-account errors[].code carries the exception's shape name instead of the service errorCode the refusal already holds

_Filed: 2026-10-29T01:26:04Z_
_Operation: BatchDeleteTaxRegistration | Class: error-response_

> _Note: this issue was identified against the code base at commit `0b0649dc1b525c86072f831fd9c0b574c3cd2f61`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BatchDeleteTaxRegistrationError$code` targets the model's free-form `ErrorCode` shape — the same shape `ConflictException$errorCode` / `ResourceNotFoundException$errorCode` and `BatchPutTaxRegistrationError$code` target. This crate states, in `ops/types/shared_types.rs`, one rule for that member: it carries a SERVICE-DEFINED code and NEVER the exception's own shape name, because a client reading it to classify a failure would take the shape name for a service code — which is why the crate goes out of its way to leave the member OFF the wire on the three exceptions that have no service code.

`BatchPutTaxRegistration` follows that rule: for a per-account validation refusal it puts the refusal's own `errorCode` in the row (e.g. `GSTExistenceBlockSetPAN`).

`BatchDeleteTaxRegistration` does the opposite. It receives the very same `ValidationException` from the shared `check_registration_deletable` helper — carrying `errorCode = FieldValidationFailed` — throws that code away and fills `code` with the literal string `"ValidationException"` (and `"ResourceNotFoundException"` for an account holding no record). So one client reading `errors[].code` across the two batch operations of one service gets a service discriminator from one and an exception shape name from the other, and the `FieldValidationFailed` code the refusal computed is never reported.

## Evidence

Crate-wide rule for this member — `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/types/shared_types.rs:17-25`:
```
// The model marks each one's `errorCode` `@required`, but it targets `ErrorCode` — a
// free-form string (length 0..50), NOT the `ValidationExceptionErrorCode` enum — and it
// carries a SERVICE-DEFINED code, never the exception's own shape name, which the wire
// already carries in `x-amzn-errortype` / `__type`. … so the three are declared unsupported and left
// off the wire instead of being filled with the shape name — which a client reading
// `errorCode` to classify a failure would take for a service code.
```

Sibling that follows it — `src/ops/batch_put_tax_registration.rs:246-250`: "The `code` of such a row is the refusal's own `errorCode`, the discriminator the service publishes for a validation refusal" and `:252-257`:
```
                errors.push(BatchPutTaxRegistrationError2 {
                    account_id: account_id.to_string(),
                    message: refusal.message,
                    code: Some(refusal.error_code),
                });
```

Handler that breaks it — `src/ops/batch_delete_tax_registration.rs:175-189`:
```
            let refused: Option<(&str, String)> = match store.registrations.get(account_id) {
                None => Some((
                    "ResourceNotFoundException",
                    crate::state::no_tax_registration().message,
                )),
                Some(stored) => crate::state::check_registration_deletable(stored)
                    .err()
                    .map(|refusal| ("ValidationException", refusal.message)),
            };
            match refused {
                Some((code, message)) => errors.push(BatchDeleteTaxRegistrationError2 {
                    account_id: account_id.to_string(),
                    message,
                    code: Some(code.to_string()),
                }),
```
`refusal.error_code` is discarded even though `check_registration_deletable` sets it — `src/state.rs:2900-2903`:
```
        match non_api_delete_country(&stored.legal_address.country_code) {
            Some(country) => Err(validation(
                ERROR_CODE_FIELD_VALIDATION_FAILED,
                "accountId",
```

Spec note: both `code` members are identical in the model (`BatchDeleteTaxRegistrationError$code` and `BatchPutTaxRegistrationError$code`, both `target: ErrorCode, length 0..50, pattern ^[\s\S]*$`, doc "The error code for an individual failure in … operation"), so nothing in the model asks the two operations to spell one refusal class two different ways.

Test coverage: `tests/all/non_api_delete_jurisdictions.rs:331-343` pins the current strings (`(carved, "ValidationException")`, `(HOLDS_NOTHING, "ResourceNotFoundException")`); `tests/all/registration_type_refusals.rs:396-406` pins the batch-put row as `ERROR_CODE_GST_EXISTENCE_BLOCK_SET_PAN`. The two pinned conventions are the inconsistency.

## Steps to Reproduce

1. POST /PutTaxRegistration with {"accountId":"000000000001","taxRegistrationEntry":{"registrationId":"11222333000181","registrationType":"CNPJ","legalName":"Acme BR","legalAddress":{"postalCode":"01310-100","countryCode":"BR"}}} to seed a registration in the Brazil carve-out.
2. POST /BatchDeleteTaxRegistration with {"accountIds":["000000000001","000000000999"]}.
3. Read errors[]: the Brazilian account is reported with code "ValidationException" and the unregistered one with "ResourceNotFoundException" — exception shape names, the one thing shared_types.rs:17-25 says this member must never carry; the FieldValidationFailed code that check_registration_deletable put on the refusal (state.rs:2900-2903) is nowhere in the response.
4. For contrast: seed 000000000001 with a GST registration, let its validation settle, then POST /BatchPutTaxRegistration with a PAN entry for it. Its errors[] row carries code "GSTExistenceBlockSetPAN" — a service code, per batch_put_tax_registration.rs:246-257.
