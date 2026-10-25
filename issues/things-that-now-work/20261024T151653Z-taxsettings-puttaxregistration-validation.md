Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# PutTaxRegistration refuses entries the model says are valid: Brazil TRNs without legalName/legalAddress, and every PAN registration

_Filed: 2026-10-24T15:16:53Z_
_Operation: PutTaxRegistration | Class: validation_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This is the INVERSE direction from the (fixed) Kenya under-validation gap: the handler rejects input the spec explicitly admits.

Expected (spec, verbatim from the model): `TaxRegistrationEntry$legalName` — "If you're setting a TRN in Brazil, you don't need to specify the legal name. For TRNs in other countries, you must specify the legal name."; `TaxRegistrationEntry$legalAddress` — "If you're setting a TRN in Brazil for the CNPJ tax type, you don't need to specify the legal address." The BatchPutTaxRegistration/PutTaxRegistration docs add the Mexico carve-out: "You do not need to provide address and legal name. These will be populated based on your tax registration number." So a BR CNPJ/CPF entry (and a Mexican entry) carrying only registrationId + registrationType (+ verificationDetails) is a 200 on the real service, with the legal name/address derived server-side and reported by GetTaxRegistration.

Also expected: `TaxRegistrationType` is a closed enum that includes `PAN` (India), and the service's own `ValidationExceptionErrorCode` carries `NonIndiaCustomerCanNotSetPAN`, i.e. an India customer CAN set a PAN registration, so a PAN entry must be storable.

Actual: `crate::state::check_tax_registration_entry` refuses, for every country alike, any entry that omits `legalName` (400, MissingInput) or `legalAddress` (400, MissingInput) — so the Brazil and Mexico carve-outs are 400s here. And `check_registration_type_members` refuses a PAN entry that CARRIES a `legalAddress`, while the required-legalAddress check refuses a PAN entry that omits one: `registrationType: "PAN"` is therefore a guaranteed 400 whatever the request contains, and no PAN registration can ever exist in this emulator. The same helper is the entry judge for BatchPutTaxRegistration, so that operation refuses the identical inputs (whole-call 400, nothing stored).

## Evidence

Spec: `smithy_query(query="shape", shape="com.amazonaws.taxsettings#TaxRegistrationEntry")` — legalName/legalAddress are NOT @required and carry the Brazil carve-out notes quoted above; `smithy_query(query="doc", shape="com.amazonaws.taxsettings#BatchPutTaxRegistration")` carries the Mexico sentences; `smithy_query(query="shape", shape="com.amazonaws.taxsettings#TaxRegistrationType")` lists PAN among the admitted values.

Handler: `src/state.rs` `check_tax_registration_entry()` lines ~1654-1691 (`Err(validation(ERROR_CODE_MISSING_INPUT, "taxRegistrationEntry.legalName", ...))` and the same for legalAddress, unconditional on country) and `check_registration_type_members()` lines ~989-1008 with `TYPES_WITH_DERIVED_LEGAL_ADDRESS = [("PAN", "India")]`; called from `src/ops/put_tax_registration.rs:261` and `src/ops/batch_put_tax_registration.rs:219`.

Tests: the module header of `src/ops/put_tax_registration.rs` (lines ~29-37) states the carve-outs are "knowingly NOT emulated", and `src/state.rs` stores `legalName`/`legalAddress` with `unwrap_or_default()` behind that refusal — so no test asserts the real-service behavior; the current refusals are what the suite pins.

## Steps to Reproduce

1. POST /PutTaxRegistration with {"accountId":"000000000001","taxRegistrationEntry":{"registrationId":"11222333000181","registrationType":"CNPJ","legalAddress":{"postalCode":"01310-000","countryCode":"BR"}}} (Brazil CNPJ, legalName omitted as the model permits) — observe 400 ValidationException/MissingInput naming LEGAL_NAME instead of a 200 with a status.
2. POST /PutTaxRegistration with {"accountId":"000000000001","taxRegistrationEntry":{"registrationId":"11222333000181","registrationType":"CNPJ"}} (both legalName and legalAddress omitted, the documented BR CNPJ case) — observe 400 instead of 200.
3. POST /PutTaxRegistration with {"accountId":"000000000001","taxRegistrationEntry":{"registrationId":"ABCDE1234F","registrationType":"PAN","legalName":"Acme India Pvt Ltd"}} — observe 400 (legalAddress required); repeat with a legalAddress added and observe 400 ("Legal address should not be provided for India PAN Tax Registration"): no PAN entry is storable in any form.
4. Repeat step 1 against /BatchPutTaxRegistration with {"accountIds":["000000000001"],...} — the same shared check refuses the whole call.
