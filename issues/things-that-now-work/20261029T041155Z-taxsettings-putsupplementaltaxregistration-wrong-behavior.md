Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d7344a303bafe2b6a404f5912cec42ec875cddf1

# A supplemental TRN rewritten with the same number in a different separator/padding form is appended as a second record under a new authorityId instead of updating the first

_Filed: 2026-10-29T04:11:55Z_
_Operation: PutSupplementalTaxRegistration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d7344a303bafe2b6a404f5912cec42ec875cddf1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutSupplementalTaxRegistration` is an add-or-update whose addressed identity is the TRN itself (its `registrationType` and `registrationId`), because the entry carries no `authorityId`. The lookup that decides "same TRN" compares the stored `registration_id` to the submitted one with a byte-exact `==`, so two spellings of ONE tax registration number — the separator form the authority prints (`CHE-123.456.789`) and the bare form (`CHE123456789`), or a value with leading/trailing whitespace the model's non-blank pattern admits — are taken for two different TRNs: a second `authorityId` is minted, a second record is inserted, `ListSupplementalTaxRegistrations` then reports the same TRN twice, and `DeleteSupplementalTaxRegistration` on one of them leaves the other behind. The "Put" becomes an append, which is precisely what the lookup's own documentation says it must not be.

The crate already holds, in two places, the rule that these spellings are one thing. `registration_number::normalised` — the function every TRN-validation judgement of this service goes through — strips ` . - / _` and upper-cases "because the authorities print the same number with and without spaces, dots, slashes and hyphens", and names `CHE-123.456.789` as its example, i.e. exactly a Swiss number, the only jurisdiction `SUPPLEMENTAL_TRN_COUNTRIES` admits a supplemental TRN for. And `is_same_exemption` — the other identity this service addresses with no server id — deliberately ignores case and padding so that "what the store treats as one type in one jurisdiction it treats as one record". The supplemental write is the one identity comparison that skips that normalisation, so one TRN judged by one structure can occupy two records.

## Evidence

The exact comparison — `services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs:803-822`:
```
/// The `authorityId` of the stored supplemental TRN with this type and
/// registration id, if the account already has one.
///
/// `SupplementalTaxRegistrationEntry` carries no `authorityId`, so the TRN
/// itself — its type and its registration id — is the identity a write
/// addresses: that is what makes `PutSupplementalTaxRegistration` "adding or
/// updating" one registration rather than appending a record per call.
pub fn find_supplemental_authority(&self, registration_type: &str, registration_id: &str) -> Option<String> {
    self.supplemental_registrations.iter().find(|(_, stored)| {
        stored.registration_type == registration_type
            && stored.registration_id == registration_id
    })
```
and its only caller, `src/ops/put_supplemental_tax_registration.rs:196-216`: "The TRN the entry names is the identity of the record: a write of one the account already holds replaces it under the `authorityId` it was first stored with, so a repeated write adds nothing." — followed by `None => { … store.mint_supplemental_authority_id() }`, which is the append.

The crate's own statement that those spellings are ONE number — `src/state/registration_number.rs:134-144`:
```
/// The submitted number with the separators a holder writes it with removed and
/// its letters upper-cased, which is the form every published structure below is
/// stated in: the authorities print the same number with and without spaces,
/// dots, slashes and hyphens (`12.345.678/0001-95`, `CHE-123.456.789`).
fn normalised(registration_id: &str) -> String {
    registration_id.chars().filter(|character| !matches!(character, ' ' | '.' | '-' | '/' | '_')).flat_map(char::to_uppercase).collect()
}
```
That example is a Swiss number, and Switzerland is the one jurisdiction the supplemental write admits — `src/state.rs:289`: `pub const SUPPLEMENTAL_TRN_COUNTRIES: &[(&str, &str)] = &[("CH", "Switzerland")];`.

The sibling identity rule this one skips — `src/state.rs:3308-3341` `is_same_exemption`: "The type and the jurisdiction are the identity a submission addresses — the request carries no exemption id — so a second submission of the same pair replaces the account's record rather than adding one beside it. … Case and padding make no second exemption either, in the type any more than in the jurisdiction: every member of the identity is compared … ignoring case and surrounding whitespace — so what the store treats as one type in one jurisdiction it treats as one record." (code: `held.trim().eq_ignore_ascii_case(description.trim())`).

The stored record is keyed by the minted id, so the second write cannot replace the first — `src/ops/put_supplemental_tax_registration.rs:218-225`: `store.supplemental_registrations.insert(authority_id.clone(), stored);`.

Test coverage: `tests/all/supplemental_tax_registrations.rs:516-564` pins the add-or-update rule only for byte-identical ids (`"CHE-123.456.789"` rewritten, `same_record = true`) and for a plainly different number (`"CHE-987.654.321"`, `same_record = false`). No row submits the same number in another separator or padding form, so nothing in the crate justifies treating them as two registrations.

## Steps to Reproduce

1. Mount the taxsettings emulator.
2. POST /PutSupplementalTaxRegistration with {"taxRegistrationEntry":{"registrationId":"CHE-123.456.789","registrationType":"VAT","legalName":"Acme AG","address":{"postalCode":"8001","countryCode":"CH"}}} and note the authorityId it answers (e.g. supplemental-trn-00000001).
3. POST /PutSupplementalTaxRegistration again with the same TRN written without the separators: {"taxRegistrationEntry":{"registrationId":"CHE123456789","registrationType":"VAT","legalName":"Acme AG","address":{"postalCode":"8001","countryCode":"CH"}}}.
4. Observe a SECOND authorityId (supplemental-trn-00000002) instead of the one the TRN was first stored under.
5. POST /ListSupplementalTaxRegistrations: two taxRegistrations entries are reported for one Swiss VAT number — the same result as repeating step 2 with a leading space (" CHE-123.456.789"), which the model's non-blank pattern admits.
6. POST /DeleteSupplementalTaxRegistration with the first authorityId: the duplicate record for the same TRN is still listed.
