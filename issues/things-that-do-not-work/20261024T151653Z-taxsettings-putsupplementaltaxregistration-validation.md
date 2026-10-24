Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# PutSupplementalTaxRegistration accepts only Switzerland and refuses every other countryCode with 400

_Filed: 2026-10-24T15:16:53Z_
_Operation: PutSupplementalTaxRegistration | Class: validation_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: the operation is modeled as "Stores supplemental tax registration for a single account" with `SupplementalTaxRegistrationEntry$address$countryCode` constrained only to two letters (`^[a-zA-Z]{2}$`) and `registrationType` limited to `VAT`. The model states no jurisdiction restriction, so a supplemental VAT registration for any jurisdiction where the caller holds one (DE, ES, GB, NL, …) is expected to be stored and then reported by ListSupplementalTaxRegistrations under a minted authorityId.

Actual: `check_supplemental_jurisdiction` compares the entry's countryCode against the emulator-invented whitelist `SUPPLEMENTAL_TRN_COUNTRIES = [("CH", "Switzerland")]` and answers 400 ValidationException/FieldValidationFailed ("Tax registration for country is not supported.") for every other country code. A client that stores a supplemental VAT number for any jurisdiction other than Switzerland gets a refusal, and ListSupplementalTaxRegistrations/DeleteSupplementalTaxRegistration can consequently only ever operate on Swiss records. The whitelist is acknowledged in the source as not derived from the model.

## Evidence

Spec: `smithy_query(query="shape", shape="com.amazonaws.taxsettings#SupplementalTaxRegistrationEntry")` / `...#SupplementalTaxRegistrationType` — countryCode is any two letters, the only type is VAT, and neither the operation doc nor any member doc names an admitted jurisdiction list; the operation's modeled errors do not describe a country restriction.

Handler: `src/ops/put_supplemental_tax_registration.rs:171` calls `crate::state::check_supplemental_jurisdiction(entry)`; `src/state.rs` lines ~136-155 (`SUPPLEMENTAL_TRN_COUNTRIES`, whose own doc comment says "WHERE THIS LIST COMES FROM: not the model … The boundary is therefore the emulator's own") and lines ~1709-1720 (the refusal).

Tests: `tests/all/supplemental_tax_registrations.rs` exercises the Swiss happy path and the refusal of other countries, i.e. the one-country whitelist is pinned; no test covers a second admitted jurisdiction.

## Steps to Reproduce

1. POST /PutSupplementalTaxRegistration with {"taxRegistrationEntry":{"registrationId":"DE123456789","registrationType":"VAT","legalName":"Acme GmbH","address":{"addressLine1":"Unter den Linden 1","city":"Berlin","postalCode":"10117","countryCode":"DE"}}}.
2. Observe 400 ValidationException / FieldValidationFailed ("Tax registration for country is not supported.") naming COUNTRY_CODE, and nothing stored.
3. Repeat with countryCode "CH" and observe 200 with a minted authorityId and status Pending — the only jurisdiction the emulator admits.
4. POST /ListSupplementalTaxRegistrations and note only the Swiss record can ever appear.
