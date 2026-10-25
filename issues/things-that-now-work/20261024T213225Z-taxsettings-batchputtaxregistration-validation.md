Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# Country requirements for Address.stateOrRegion (Canada, India, UAE, Romania, Brazil CPF) and Canada's isResellerAccount rule are never enforced

_Filed: 2026-10-24T21:32:25Z_
_Operation: BatchPutTaxRegistration | Class: validation_

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both writers of a `TaxRegistrationEntry` (`BatchPutTaxRegistration` and `PutTaxRegistration`) judge an entry through the shared `crate::state::check_tax_registration_entry` -> `check_country_requirements` / `COUNTRY_RULES` table. That table carries one row per country requirement the operation documentation states (BD, GE, ID, KE, MY, MX, NP, SA, KR, ES, TR, UA, FR, MC), but it is missing two requirements the model states elsewhere, so entries AWS refuses are accepted here with 200 and stored.

(a) `Address$stateOrRegion` is documented as "This field is only required for Canada, India, United Arab Emirates, Romania, and Brazil (CPF). It is optional for all other countries." There is no `COUNTRY_RULES` row for CA / IN / AE / RO, and no CPF-specific rule for BR.
Expected: a `taxRegistrationEntry` whose `legalAddress.countryCode` is CA, IN, AE or RO (or BR with `registrationType: CPF`) and whose address omits `stateOrRegion` is refused with the modelled `ValidationException` (400, errorCode `MissingInput`, `fieldList` naming the parameter), and nothing is stored.
Actual: the entry is accepted — `BatchPutTaxRegistration` writes it to every account it names and answers 200 with `status: Pending`, and a later `GetTaxRegistration`/`ListTaxRegistrations` reports an address with no `stateOrRegion`.

(b) `CanadaAdditionalInfo$isResellerAccount` is documented as "The value for this parameter must be `true` if the `provincialSalesTaxId` value is provided for a TRN in British Columbia, Saskatchewan, or Manitoba provinces." Nothing in the emulator relates the two members; a Canadian entry carrying `provincialSalesTaxId` for one of those provinces with `isResellerAccount` absent or `false` is accepted.

This is the OPPOSITE direction from the existing open issue `taxsettings-puttaxregistration-validation` (which is about entries the emulator over-refuses: the Brazil/Mexico legalName/legalAddress carve-outs and PAN); it is filed against `BatchPutTaxRegistration` because that operation carries no validation issue yet, but the fix belongs in the shared `COUNTRY_RULES` table and therefore closes the same hole on `PutTaxRegistration`.

## Evidence

Spec: `smithy_query(query="doc", shape="com.amazonaws.taxsettings#Address", member="stateOrRegion")` -> "The state, region, or province that the address is located. This field is only required for Canada, India, United Arab Emirates, Romania, and Brazil (CPF). It is optional for all other countries."; `smithy_query(query="shape", shape="com.amazonaws.taxsettings#CanadaAdditionalInfo")` -> `isResellerAccount` doc "The value for this parameter must be true if the provincialSalesTaxId value is provided for a TRN in British Columbia, Saskatchewan, or Manitoba provinces." `stateOrRegion` is not `@required` on the `Address` shape, so the boundary validator cannot refuse its absence — it is a per-country business rule a handler must apply, exactly like Saudi Arabia's `addressLine3` and Turkey's `districtOrCounty`, which ARE in the table.

Handler: `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/batch_put_tax_registration.rs:219` calls `crate::state::check_tax_registration_entry(entry)` (same call at `src/ops/put_tax_registration.rs:261`). `src/state.rs` `COUNTRY_RULES` (lines ~1257-1612) contains rows only for BD, GE, ID (x4), KE (x2), MY (x4), MX, NP, SA (`legalAddress.addressLine3`), KR (x3), ES (x2), TR (x4), UA, FR, MC — no CA, IN, AE or RO row, no CPF/BR row, and no rule relating `canadaAdditionalInfo.provincialSalesTaxId` to `isResellerAccount`.

Tests: no test asserts the requirement; the suite pins the current acceptance — `tests/all/tax_registration_lifecycle.rs:184-195` stores a Brazil CPF registration whose `legalAddress` is `{"postalCode":"22041-001","countryCode":"BR"}` (no `stateOrRegion`) and asserts a 200 round-trip, and `tests/all/country_rules.rs` / `country_requirements.rs` cover only the countries already in the table.

## Steps to Reproduce

1. POST /BatchPutTaxRegistration with {"accountIds":["000000000001"],"taxRegistrationEntry":{"registrationId":"29ABCDE1234F1Z5","registrationType":"GST","legalName":"Acme India Private Limited","legalAddress":{"addressLine1":"1 Residency Road","city":"Bengaluru","postalCode":"560025","countryCode":"IN"}}} — stateOrRegion omitted for India. Observe 200 with status Pending and an empty errors list; AWS answers 400 ValidationException (MissingInput) naming the state/region parameter.
2. POST /GetTaxRegistration with {"accountId":"000000000001"} — observe the registration was stored, with legalAddress carrying no stateOrRegion.
3. Repeat step 1 with countryCode CA, AE and RO (and with countryCode BR + registrationType CPF): every one is accepted.
4. POST /BatchPutTaxRegistration with a Canadian entry {"legalAddress":{"stateOrRegion":"British Columbia","postalCode":"V6B 1A1","countryCode":"CA"},"additionalTaxInformation":{"canadaAdditionalInfo":{"provincialSalesTaxId":"PST-1234567"}}} and no isResellerAccount — observe 200 instead of the documented refusal.
5. Repeat step 1 against /PutTaxRegistration: the same shared check accepts it.
