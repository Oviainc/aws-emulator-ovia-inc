Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6c829b3943454ada5df5dfe2630a7a4c6694ce1c

# legalAddress.addressLine3 is accepted and stored for every country, though the model says only Saudi Arabia accepts it

_Filed: 2026-10-29T06:35:30Z_
_Operation: PutTaxRegistration | Class: validation_

> _Note: this issue was identified against the code base at commit `6c829b3943454ada5df5dfe2630a7a4c6694ce1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Address$addressLine3` carries a two-part documented rule: the API accepts the member ONLY for Saudi Arabia, and a Saudi registration MUST carry it. This crate implements only the second half.

`check_tax_registration_entry` (src/state.rs:2970-3045) never looks at which country submitted an `addressLine3`: the only two places the member is touched are `check_address`, which checks non-blankness alone, and the one `COUNTRY_RULES` row keyed on `SA`, which REQUIRES it there. So an entry whose `legalAddress.countryCode` is anything else — JP, NL, BR, MY, TR, KE, CA — is stored with the submitted `addressLine3`, and `GetTaxRegistration` / `ListTaxRegistrations` report it back, even though the model says the API does not accept the parameter for that country.

This is not a rule class the crate is missing machinery for: `COUNTRY_RULES` already carries a "member that must NOT be entered here" row (Turkey's `taxOffice` for the `Individual` sector, refused with `FieldValidationFailed` / `ADDITIONAL_INFO_INVALID`), and the crate's own header for the member-derived rows says those rows exist precisely because "a member one country's registration must carry, or must carry a given value … no constraint on the shape can express because the shape is shared by every country" — the `addressLine3` sentence is of that same kind, and the crate only transcribed its requiring half.

The same shared address path serves `PutSupplementalTaxRegistration` (`check_address("taxRegistrationEntry.address", …)`), so a supplemental Swiss TRN carrying `addressLine3` is admitted too.

## Evidence

Model sentence (smithy_query(query="shape", shape="Address") -> member `addressLine3`), which states the behaviour itself:
```
"addressLine3": {"target": "AddressLine3", ... "doc": "<p> The third line of the address, if applicable. Currently, the Tax Settings API accepts the <code>addressLine3</code> parameter only for Saudi Arabia. When you specify a TRN in Saudi Arabia, you must enter the <code>addressLine3</code> and specify the building number for the address. For example, you might enter <code>1234</code>.</p>"}
```

Only the requiring half is implemented — `services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:2467-2480`:
```
    // Saudi Arabia: "For address, you must specify addressLine3."
    CountryRule {
        country_code: "SA",
        member_path: "taxRegistrationEntry.legalAddress.addressLine3",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in Saudi Arabia must specify legalAddress.addressLine3.",
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            entry.legal_address.as_ref().is_some_and(|address| address.address_line3.is_some())
        },
    },
```
and the crate's own summary of the member, `src/state.rs:149-152`, names only that half: "Country rules that make a member REQUIRED (Saudi Arabia's `addressLine3`, Turkey's `districtOrCounty`, Kenya's `personType`, …)".

Nothing refuses the member elsewhere — `src/state.rs:1877-1893` is the only other code that reads it, and it checks blankness alone:
```
pub fn check_address(member_path: &str, address: &Address) -> Result<(), ValidationException> {
    let members: [(&str, Option<&String>); 7] = [
        ("addressLine1", address.address_line1.as_ref()),
        ...
        ("addressLine3", address.address_line3.as_ref()),
```

The crate already implements the "must not be entered" class — `src/state.rs:2580-2589` (Turkey's `taxOffice` for the `Individual` sector):
```
    CountryRule {
        country_code: "TR",
        member_path: "additionalTaxInformation.turkeyAdditionalInfo.taxOffice",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: ADDITIONAL_INFO_INVALID,
        applies: turkey_sector_is_individual,
        satisfied: |entry: &TaxRegistrationEntry| {
            turkey_member(entry, |turkey| turkey.tax_office.as_ref()).is_none()
        },
    },
```

Test coverage: the acceptance is pinned only incidentally, as a generic "every member of the shape" address rather than against the quoted sentence — `tests/all/declared_member_defaults.rs:126-144` submits `"addressLine3": "1234"` with `"countryCode": "JP"` and again with an `NL` address, and `tests/all/derived_address_precedence.rs:67-88` calls its fully-populated form (carrying `"addressLine3": "Example Block"`) "every member it accepts" for BR, MY, TR, KE and CA. The Saudi half is pinned by `tests/all/country_requirements.rs:506-531` (`missing("ADDRESS_LINE3")`); no test pins that a non-Saudi `addressLine3` must be accepted for a stated reason.

## Steps to Reproduce

1. POST /PutTaxRegistration with {"accountId":"000000000001","taxRegistrationEntry":{"registrationId":"VAT-0000000001","registrationType":"VAT","legalName":"Example Holdings","legalAddress":{"addressLine1":"1 Example Street","addressLine3":"1234","city":"Tokyo","postalCode":"1500001","countryCode":"JP"}}}.
2. Observe HTTP 200 with {"status":"Pending"} — the member the model says the API accepts only for Saudi Arabia was accepted for Japan.
3. POST /GetTaxRegistration with {"accountId":"000000000001"} and read taxRegistration.legalAddress.addressLine3: it reads back "1234", so the unaccepted member was persisted and is reported.
4. For contrast, the crate does refuse a member one country may not enter: POST /PutTaxRegistration for a Turkish address with "sector":"Individual" and additionalTaxInformation.turkeyAdditionalInfo.taxOffice set -> 400 ValidationException / FieldValidationFailed (src/state.rs:2580-2589).
