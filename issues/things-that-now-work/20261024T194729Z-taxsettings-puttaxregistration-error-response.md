Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7cd33b8d616838a49131331fab4e1704473d18dc

# The PAN-specific ValidationException error codes NonIndiaCustomerCanNotSetPAN and GSTExistenceBlockSetPAN are never produced

_Filed: 2026-10-24T19:47:29Z_
_Operation: PutTaxRegistration | Class: error-response_

> _Note: this issue was identified against the code base at commit `7cd33b8d616838a49131331fab4e1704473d18dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ValidationExceptionErrorCode` enumerates seven service-defined codes: `MalformedToken | ExpiredToken | InvalidToken | FieldValidationFailed | MissingInput | NonIndiaCustomerCanNotSetPAN | GSTExistenceBlockSetPAN`. The last two exist because AWS answers the two PAN-specific refusals of `PutTaxRegistration` (and its batch sibling) with them: a customer outside India submitting a `registrationType` of `PAN` gets `NonIndiaCustomerCanNotSetPAN`, and an account that already holds a GST registration being given a PAN gets `GSTExistenceBlockSetPAN`.

Expected: a `PutTaxRegistration` whose entry sets `registrationType: "PAN"` for a non-India customer is refused 400 `ValidationException` with `errorCode: "NonIndiaCustomerCanNotSetPAN"`; a PAN write against an account whose stored registration is a `GST` one is refused 400 `ValidationException` with `errorCode: "GSTExistenceBlockSetPAN"`. A client switching on `errorCode` (the discriminator this service publishes precisely so a caller need not parse the message) can tell these two situations from an ordinary field violation.

Actual: the emulator only ever emits four of the seven codes — `src/state.rs` defines constants for `MissingInput`, `FieldValidationFailed`, `MalformedToken` and `InvalidToken` and nothing else. Every PAN refusal collapses onto `FieldValidationFailed`/`MissingInput`: `check_registration_type_members()` refuses any entry whose `registrationType` is `PAN` and that carries a `legalAddress` with `errorCode: FieldValidationFailed`, `fieldList: [{"name": "LEGAL_ADDRESS"}]` and the message "Legal address should not be provided for India PAN Tax Registration" — the same answer for a customer in Germany as for one in India — and a PAN entry that omits `legalAddress` is refused `MissingInput`/`LEGAL_ADDRESS` instead. The GST-blocks-PAN rule is not implemented at all, even though the state holds each account's stored `registrationType` (`StoredTaxRegistration.registration_type`), so `GST` followed by `PAN` is distinguishable from state; the existence of a prior GST record has no effect on the refusal a client sees.

## Evidence

Spec: `smithy_query(query="shape", shape="ValidationExceptionErrorCode")` -> values include `NonIndiaCustomerCanNotSetPAN` and `GSTExistenceBlockSetPAN`; `ValidationException$errorCode` is required on every refusal this service answers. `smithy_query(query="shape", shape="TaxRegistrationType")` -> `PAN` and `GST` are both modeled registration types, so both antecedents of these codes are expressible on the wire. `src/state.rs` lines ~157-167 itself quotes the full enum while defining constants for only four of its values, and lines ~121-134 note that "the model's own `ValidationExceptionErrorCode` enum carries `NonIndiaCustomerCanNotSetPAN`, so the type is India-only" — the inference is drawn but the code is never emitted.

Handler: `src/ops/put_tax_registration.rs` `handle()` line ~261 delegates every entry refusal to `crate::state::check_tax_registration_entry`, which calls `check_registration_type_members` (`src/state.rs` line ~989, errorCode `FieldValidationFailed`) and then `check_country_requirements` (every `COUNTRY_RULES` row carries `MissingInput` or `FieldValidationFailed`). `grep -n "NonIndiaCustomerCanNotSetPAN\|GSTExistenceBlockSetPAN" src/` finds no producer. `BatchPutTaxRegistration` shares the same checker, so it is equally affected.

Tests: `tests/all/fidelity_rules.rs::a_type_whose_address_is_derived_refuses_a_submitted_legal_address` and `tests/all/country_rules.rs` assert the `FieldValidationFailed`/`MissingInput` outcomes only; no test exercises either PAN-specific code, so the gap is untested rather than asserted-in.

## Steps to Reproduce

1. POST /PutTaxRegistration with {"accountId":"000000000001","taxRegistrationEntry":{"registrationId":"ABCDE1234F","registrationType":"PAN","legalName":"Acme GmbH","legalAddress":{"postalCode":"10115","countryCode":"DE"}}} — a non-India customer setting a PAN. Observe 400 with errorCode "FieldValidationFailed" and fieldList [{"name":"LEGAL_ADDRESS"}]; AWS answers errorCode "NonIndiaCustomerCanNotSetPAN".
2. POST /PutTaxRegistration with a valid India GST entry: {"accountId":"000000000002","taxRegistrationEntry":{"registrationId":"29ABCDE1234F1Z5","registrationType":"GST","legalName":"Acme India Pvt Ltd","legalAddress":{"addressLine1":"1 MG Road","city":"Bengaluru","postalCode":"560001","countryCode":"IN","stateOrRegion":"Karnataka"}}} and observe 200.
3. Wait for the registration to settle, then POST /PutTaxRegistration for the same accountId with {"taxRegistrationEntry":{"registrationId":"ABCDE1234F","registrationType":"PAN","legalName":"Acme India Pvt Ltd"}}: observe a 400 whose errorCode is "MissingInput" (for the absent legalAddress) rather than "GSTExistenceBlockSetPAN"; the pre-existing GST record is not consulted.
4. Grep the response bodies of every refusal the service can produce: no request sequence makes errorCode take either PAN-specific value, so a client branching on them never takes those branches.
