Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d0484a4a393e00c66ab2f2d9da27b0764a19da5b

# A TaxRegistrationEntry carrying several additionalTaxInformation country blocks (or one for another country, or an empty object) is stored instead of refused

_Filed: 2026-10-29T00:53:42Z_
_Operation: PutTaxRegistration | Class: validation_

> _Note: this issue was identified against the code base at commit `d0484a4a393e00c66ab2f2d9da27b0764a19da5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states, on the `additionalTaxInformation` shape itself, that exactly one of its country blocks may be specified and that the value cannot be empty, plus a note that the block specified must match the country the TRN is held in. The crate's entry judge, `crate::state::check_tax_registration_entry` — the one place `PutTaxRegistration` and `BatchPutTaxRegistration` decide whether an entry is storable — never looks at HOW MANY blocks the object carries, nor at whether the block belongs to the entry's own jurisdiction. `check_country_requirements` only walks the rows of `COUNTRY_RULES` whose `country_code` equals the entry's own country, so a block for any other country is simply never examined, and an `additionalTaxInformation` object with no block at all is admitted for every country that has no `applies: always` row.

Concretely: an entry held in Poland (`legalAddress.countryCode = "PL"`, `registrationType = "NIP"`) that carries BOTH `polandAdditionalInfo` AND `canadaAdditionalInfo` (or that carries only `canadaAdditionalInfo`, or an empty `additionalTaxInformation: {}`) is accepted with 200 / `Pending`, persisted, and then reported back with every submitted block, because `additional_info_response` copies each block one by one onto the response shape. The service's own refusal for this class of problem already exists in the crate (`ADDITIONAL_INFO_INVALID` + `ERROR_CODE_FIELD_VALIDATION_FAILED`, used by every other `additionalTaxInformation` rule), so the rule is unenforced rather than unreachable.

## Evidence

Smithy, `com.amazonaws.taxsettings#AdditionalInfoRequest` (shape documentation, via smithy_query): "Additional tax information associated with your tax registration number (TRN). Depending on the TRN for a specific country, you might need to specify this information when you set your TRN." / "You can only specify one of the following parameters and the value can't be empty." / note: "The parameter that you specify must match the country for the TRN, if available. For example, if you set a TRN in Canada for specific provinces, you must also specify the canadaAdditionalInfo parameter."

The handler side — `services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:2756` `pub fn check_tax_registration_entry(` applies, in order, `check_registration_type_members(entry)?` (2764), `check_registration_type_jurisdiction(entry)?` (2770), the legalAddress/legalName carve-outs (2780-2808), `check_not_blank` / `check_address` (2810-2819) and finally `check_country_requirements(entry)` (2824). No step counts the blocks of `additionalTaxInformation`; the only accessor of that object is `state.rs:2092` `fn additional_info(entry: &TaxRegistrationEntry) -> Option<&AdditionalInfoRequest> { entry.additional_tax_information.as_ref() }`, used solely to fetch ONE named block.

`services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:2721-2736`:
```
pub fn check_country_requirements(entry: &TaxRegistrationEntry) -> Result<(), ValidationException> {
    let Some(country_code) = entry_country_code(entry) else { return Ok(()); };
    for rule in COUNTRY_RULES {
        if !rule.country_code.eq_ignore_ascii_case(country_code) { continue; }
```
— so a `canadaAdditionalInfo` block on a Polish entry is skipped by every row.

The table's own header claims completeness and names its only deliberate omissions, and this rule is not among them — `state.rs:2250-2270`: "Every per-country business rule `PutTaxRegistration`'s model documentation states for `taxRegistrationEntry` ... followed by the per-country requirements the model states on the entry's own MEMBERS rather than in that list (`Address$stateOrRegion`, `CanadaAdditionalInfo$isResellerAccount`) ... Two documented rules are deliberately NOT rows here ... Brazil's \"complete the registration process in the console first\" ... and Turkey's \"if your `sector` is `Business` and you're subject to KDV tax ...\"".

The accepted entry is then reported with every block it carried — `state.rs:1322-1353` `fn additional_info_response(request: &AdditionalInfoRequest) -> AdditionalInfoResponse { AdditionalInfoResponse { malaysia_additional_info: ..., canada_additional_info: request.canada_additional_info.clone(), ... poland_additional_info: request.poland_additional_info.clone(), ... } }`.

Call sites of the unenforcing judge: `src/ops/put_tax_registration.rs:292` `crate::state::check_tax_registration_entry(entry)` and `src/ops/batch_put_tax_registration.rs:224` (same call).

Test coverage: every entry in the crate's suites carries at most ONE country block, and always the one matching its address — `tests/all/tax_registration_lifecycle.rs:252` (MY -> `malaysiaAdditionalInfo`), `:298` (ES -> `spainAdditionalInfo`), `:312` (PL -> `polandAdditionalInfo`), `tests/all/declared_member_defaults.rs:363,376,393,410`. No test pins the acceptance of a multi-block, foreign-block or empty `additionalTaxInformation` object, so nothing in the crate justifies the divergence.

## Steps to Reproduce

1. Mount the taxsettings emulator and call `PutTaxRegistration` with `{"accountId":"000000000009","taxRegistrationEntry":{"registrationId":"1234563218","registrationType":"NIP","legalName":"Przyklad Sp. z o.o.","sector":"Business","legalAddress":{"postalCode":"00-001","countryCode":"PL"},"additionalTaxInformation":{"polandAdditionalInfo":{"taxRegistrationNumberType":"LocalRegistrationNumber"},"canadaAdditionalInfo":{"provincialSalesTaxId":"PST-1234-5678"}}}}`
2. Observe HTTP 200 with `status: "Pending"` — two country blocks were accepted although the model says only one of the parameters may be specified
3. Call `GetTaxRegistration` with `{"accountId":"000000000009"}` and observe `taxRegistration.additionalTaxInformation` carrying BOTH `polandAdditionalInfo` and `canadaAdditionalInfo`
4. Repeat the write with only `canadaAdditionalInfo` for the same Polish address, and again with `"additionalTaxInformation": {}`: both are also accepted with 200 / `Pending`, although the block does not match the country of the TRN and the value is empty
