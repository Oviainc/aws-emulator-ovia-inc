Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: ad9d9cbded3020216cba71d97cb223a025446b5b

# Every stored TRN reads back status Rejected 500 ms after it was written; Verified is unreachable

_Filed: 2026-10-24T17:05:36Z_
_Operation: GetTaxRegistration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ad9d9cbded3020216cba71d97cb223a025446b5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a tax registration number that the service accepts and stores reads back as `Verified` once its validation completes (the normal outcome for a well-formed TRN in a supported country), with `Pending` reported while validation is outstanding and `Rejected` reserved for a number whose validation actually failed. A client workflow of PutTaxRegistration -> GetTaxRegistration therefore ends with `{"taxRegistration":{...,"status":"Verified"}}` for a good TRN.

Actual: the emulator hard-codes the terminal status of EVERY registration to `Rejected`. `crate::state::SETTLED_REGISTRATION_STATUS = STATUS_REJECTED`, and both `StoredTaxRegistration::settle` and `StoredSupplementalRegistration::settle` assign that constant once `validation_due_at` (write time + 500 ms `TRN_VALIDATION_DELAY`) has passed. `GetTaxRegistration` calls `store.settle_due(Instant::now())` before projecting the record, so any read taken more than 500 ms after the write reports `Rejected` for a TRN the service itself just accepted and persisted. Nothing inspects the submitted number, so the status carries no information: a valid German VAT number, a valid Kenyan PIN and a nonsense-but-schema-valid string all end up `Rejected`.

The divergence is observable on every read path that reports a registration status: `GetTaxRegistration` (`TaxRegistration$status`), `ListTaxRegistrations` (`AccountDetails$taxRegistration.status`) and `ListSupplementalTaxRegistrations` (`SupplementalTaxRegistration$status`), all of which call `settle_due` first. `Verified` — one of the four modeled `TaxRegistrationStatus` values and the one the model names first for a processed TRN — is unreachable in this emulator by any request sequence, so a client that polls for a terminal status and branches on it takes the failure branch for every registration it successfully created.

A faithful emulator either settles an accepted registration to `Verified` (it accepted and stored it, so its own validation succeeded) or leaves it `Pending`; reporting `Rejected` contradicts the 200 the write returned.

## Evidence

Spec: `PutTaxRegistration$status` is documented as "The status of your TRN stored in the system after processing. Based on the validation occurring on the TRN, the status can be Verified, Pending or Rejected" (`smithy_query(query="doc", shape="com.amazonaws.taxsettings#PutTaxRegistrationResponse", member="status")`), and `TaxRegistrationStatus` enumerates `Deleted | Pending | Rejected | Verified`. `TaxRegistration$status` is `@required` on the shape `GetTaxRegistration` returns, so the value is always on the wire. Well-established service behavior: a TRN that AWS accepts and validates is reported Verified; Rejected is the outcome for a number that failed validation.

Handler/state: `src/state.rs` — `pub const SETTLED_REGISTRATION_STATUS: &str = STATUS_REJECTED;` (approx. line 53), with `STATUS_VERIFIED` documented as "Never reached"; `StoredTaxRegistration::settle()` (approx. line 683) and `StoredSupplementalRegistration::settle()` (approx. line 573) both assign it; `ComAmazonawsTaxsettingsTaxsettingsState::settle_due()` (approx. line 283) applies it to every record. `src/ops/get_tax_registration.rs` `handle()` line ~156 calls `settle_due` before projecting; `src/ops/list_tax_registrations.rs` line ~190 and `src/ops/list_supplemental_tax_registrations.rs` line ~183 do the same.

Tests: `tests/all/fidelity_rules.rs::a_submitted_trn_is_pending_until_its_validation_is_due_then_settles` pins the current Rejected outcome rather than a Verified one, so the gap is asserted-in rather than untested — a fixer must update that assertion (and any sibling assertion in `tests/all/pending_trn_conflict.rs`, which relies on a settled record no longer blocking a rewrite).

## Steps to Reproduce

1. POST /PutTaxRegistration with {"accountId":"000000000001","taxRegistrationEntry":{"registrationId":"DE123456789","registrationType":"VAT","legalName":"Acme GmbH","legalAddress":{"addressLine1":"Unter den Linden 1","city":"Berlin","postalCode":"10117","countryCode":"DE"}}} and observe 200 {"status":"Pending"}.
2. Wait more than 500 ms (the emulator's TRN_VALIDATION_DELAY).
3. POST /GetTaxRegistration with {"accountId":"000000000001"} and observe {"taxRegistration":{...,"status":"Rejected"}} for the TRN the service just accepted; AWS reports Verified for a TRN that passes validation.
4. POST /ListTaxRegistrations with {} and observe the same account reported with status Rejected.
5. Repeat with any other country/registrationType combination the emulator accepts: every registration settles to Rejected, so no request sequence can make any read report Verified.
6. POST /PutSupplementalTaxRegistration with a valid Swiss VAT entry, wait 500 ms, then POST /ListSupplementalTaxRegistrations: the supplemental record is reported Rejected as well.
