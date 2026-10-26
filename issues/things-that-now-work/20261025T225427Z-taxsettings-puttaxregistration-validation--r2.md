Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3934dd8dfa9c40453bce3aa6cb9dd67d7ad9e6cc

# Malaysia's documented registrationType/sector pairing is never enforced: a Business NRIC (or Individual TIN/SST) registration is stored

_Filed: 2026-10-25T22:54:27Z_
_Operation: PutTaxRegistration | Class: validation_

> _Note: this issue was identified against the code base at commit `3934dd8dfa9c40453bce3aa6cb9dd67d7ad9e6cc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model's Malaysia block for `PutTaxRegistration`/`BatchPutTaxRegistration` states two separate rules: which `sector` values are valid, AND which `registrationType` value goes with which sector. `crate::state::COUNTRY_RULES` implements the first rule and only the VALUE SET half of the second: the `MY` `registrationType` row admits any of `NRIC | TIN | SST` regardless of the entry's `sector`. So an entry with `legalAddress.countryCode = "MY"`, `sector = "Business"` and `registrationType = "NRIC"` (NRIC is documented as the individual type), or `sector = "Individual"` with `registrationType = "TIN"`/`"SST"`, passes `check_tax_registration_entry` and is persisted with 200 / `Pending` instead of being refused with the modelled `ValidationException`.

This is not a rule the crate never attempted: the crate already encodes which Malaysian types are the business ones (`MALAYSIA_BUSINESS_REGISTRATION_TYPES`) and already reads `sector` through its own `sector()` accessor for the Turkey and Nepal/Ukraine/Malaysia rows — it simply never compares the two. The `COUNTRY_RULES` header also claims the table is complete except for two explicitly named omissions, and this pairing is a third, unacknowledged one.

Both write paths are affected, because both go through the one shared judge (`check_tax_registration_entry`).

## Evidence

MODEL (smithy_query query="doc" shape="com.amazonaws.taxsettings#PutTaxRegistration"), the Malaysia block:
  "The sector valid values are <code>Business</code> and <code>Individual</code>."
  "<code>RegistrationType</code> valid values are <code>NRIC</code> for individual, and TIN and sales and service tax (SST) for Business."

CODE — the only `MY` row about `registrationType`, which ignores `sector` entirely:
services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:1811-1821
```
    CountryRule {
        country_code: "MY",
        member_path: "taxRegistrationEntry.registrationType",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: "The valid registrationType values for a tax registration in Malaysia are NRIC, \
                  TIN and SST.",
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            MALAYSIA_REGISTRATION_TYPES.contains(&entry.registration_type.as_str())
        },
    },
```
with services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:1413-1416
```
/// The `TaxRegistrationType` values the model documents as valid for Malaysia
/// ("`RegistrationType` valid values are `NRIC` for individual, and TIN and
/// sales and service tax (SST) for Business").
pub const MALAYSIA_REGISTRATION_TYPES: &[&str] = &["NRIC", "TIN", "SST"];
```
— the quoted sentence states the pairing, the constant keeps only the union.

THE CRATE ALREADY KNOWS THE PAIRING — services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:1418-1421
```
/// The Malaysian registration types the model documents as the business ones,
/// for which "you must specify a `businessRegistrationNumber` in
/// `MalaysiaAdditionalInfo`".
pub const MALAYSIA_BUSINESS_REGISTRATION_TYPES: &[&str] = &["TIN", "SST"];
```
and it reads `sector` for other rows — services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:1553-1556
```
/// `taxRegistrationEntry.sector`, if the entry carries one.
fn sector(entry: &TaxRegistrationEntry) -> Option<&str> {
    entry.sector.as_deref()
}
```

THE COMPLETENESS CLAIM THIS BREAKS — services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:1672-1687
```
/// Every per-country business rule `PutTaxRegistration`'s model documentation
/// states for `taxRegistrationEntry`, in the order that documentation lists the
/// countries
...
/// Two documented rules are deliberately NOT rows here, because the fact they
/// turn on is not in the request and this service models no API that supplies
/// one: Brazil's "complete the registration process in the console first" ...
/// and Turkey's "if your `sector` is `Business` and you're subject to KDV tax,
/// you must specify your industry" ...
```
The Malaysia pairing turns on `sector` and `registrationType`, both of which ARE in the request, so it is not one of the two acknowledged exceptions.

TEST COVERAGE — the completeness test cannot catch a missing row: services/service-com-amazonaws-taxsettings-taxsettings/tests/all/country_requirements.rs:1116-1152 asserts only that every row ALREADY in `COUNTRY_RULES` is exercised ("for rule in COUNTRY_RULES { ... assert!(exercised.contains(&wanted) ..."), so a documented rule with no row is invisible to it. The shared judge both writers use is services/service-com-amazonaws-taxsettings-taxsettings/src/ops/put_tax_registration.rs:271 and services/service-com-amazonaws-taxsettings-taxsettings/src/ops/batch_put_tax_registration.rs:219 (`crate::state::check_tax_registration_entry(entry)`), so the gap is shared by `BatchPutTaxRegistration` too (tests/all/country_rules.rs:241 `both_writers_judge_one_entry_by_the_same_country_rules`).

## Steps to Reproduce

1. POST /PutTaxRegistration with body: {"accountId":"000000000000","taxRegistrationEntry":{"registrationId":"MY-123456","registrationType":"NRIC","sector":"Business","legalName":"Acme Sdn Bhd","legalAddress":{"addressLine1":"1 Jalan","city":"Kuala Lumpur","postalCode":"50000","countryCode":"MY"}}}
2. Observe HTTP 200 with {"status":"Pending"} — NRIC is documented as the individual registration type, yet the entry declares sector Business and is stored
3. POST /GetTaxRegistration with {"accountId":"000000000000"} and observe the Business/NRIC registration reads back; expected the write to have been refused with 400 ValidationException naming the offending parameter
4. Repeat with {"registrationType":"TIN","sector":"Individual", ... ,"additionalTaxInformation":{"malaysiaAdditionalInfo":{"businessRegistrationNumber":"BRN-1"}}} — also accepted with 200 Pending, although TIN is documented as a Business type
