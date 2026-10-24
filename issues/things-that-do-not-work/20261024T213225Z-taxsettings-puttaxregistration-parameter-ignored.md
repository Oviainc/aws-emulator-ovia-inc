Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# The model-declared default "Unknown" for Address.addressLine1 and Address.city is never applied, so stored addresses read back with those members absent

_Filed: 2026-10-24T21:32:25Z_
_Operation: PutTaxRegistration | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Address$addressLine1` and `Address$city` carry `@default: "Unknown"` in the model (with `@addedDefault`, which keeps them nullable for older clients but still declares the value the service substitutes when the member is absent). Only `postalCode` and `countryCode` are `@required`, so an address submitted with just those two is wire-valid and common (several tests in this crate do exactly that).

Expected: a registration written with `legalAddress: {"postalCode":"10115","countryCode":"DE"}` is stored with the declared defaults materialised, so `GetTaxRegistration` reports `legalAddress` carrying `addressLine1: "Unknown"` and `city: "Unknown"`, and `ListTaxRegistrations` reports the same pair inside `accountDetails[].accountMetaData.address`.

Actual: the write stores the address exactly as received (`StoredTaxRegistration::from_entry` -> `entry.legal_address.clone()`), the generated struct keeps both members as `Option<String>`, and nothing applies the declared default — so every read reports the address with `addressLine1` and `city` missing from the JSON body. The same shared `Address` path is used by `BatchPutTaxRegistration` and by `PutSupplementalTaxRegistration` (whose address is reported by `ListSupplementalTaxRegistrations`), so a fix in one place covers all three.

## Evidence

Spec: `smithy_query(query="shape", shape="com.amazonaws.taxsettings#Address")` -> `addressLine1 {"default": "Unknown", "addedDefault": true}` and `city {"default": "Unknown", "addedDefault": true}`; `postalCode`/`countryCode` are the only `@required` members. The crate's own `OPERATIONS.md` preamble states the rule for this case: "an optional member can still show a declared default for a handler to apply".

Handler/state: `src/ops/put_tax_registration.rs:272` -> `state.lock().store_registration(...)`; `src/state.rs` `StoredTaxRegistration::from_entry` (~line 656) does `entry.legal_address.clone().unwrap_or_default()` and stores the `Address` verbatim; `src/ops/types/shared_types.rs:118-129` generates `address_line1: Option<String>` / `city: Option<String>` with no default fallback, and `vendor/smithy-macros/src/lib.rs:2137-2148` (`usable_default`) deliberately returns `None` for an `@addedDefault` member, so the boundary deserializer does not materialise it either — nothing in the request path does.

Tests: no test asserts the defaults; the suite pins the current omission — `tests/all/tax_registration_lifecycle.rs:286` ("a stored tax registration is reported, member for member, as it was submitted") round-trips `legalAddress: {"postalCode":"10115","countryCode":"DE"}` and compares it to the submitted object, and `tests/all/batch_tax_registration.rs:148` asserts each member "must be what was submitted".

## Steps to Reproduce

1. POST /PutTaxRegistration with {"accountId":"000000000001","taxRegistrationEntry":{"registrationId":"DE123456789","registrationType":"VAT","legalName":"Acme GmbH","legalAddress":{"postalCode":"10115","countryCode":"DE"}}} — addressLine1 and city omitted, as the model permits.
2. POST /GetTaxRegistration with {"accountId":"000000000001"} — observe taxRegistration.legalAddress == {"postalCode":"10115","countryCode":"DE"} with no addressLine1 and no city; the model's declared default for both is "Unknown".
3. POST /ListTaxRegistrations with {} — observe accountDetails[0].accountMetaData.address shows the same two members missing.
4. POST /PutSupplementalTaxRegistration with a Swiss entry whose address omits addressLine1/city, then POST /ListSupplementalTaxRegistrations — the same omission appears in taxRegistrations[].address.
