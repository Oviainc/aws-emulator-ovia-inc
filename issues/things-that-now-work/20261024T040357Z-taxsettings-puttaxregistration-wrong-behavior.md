Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 733b41f401fdbdc43a4f3ac4e403542c858679fd

# PutTaxRegistration overwrites a Pending TRN instead of answering ConflictException

_Filed: 2026-10-24T04:03:57Z_
_Operation: PutTaxRegistration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `733b41f401fdbdc43a4f3ac4e403542c858679fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation's own model documentation states the rule plainly: "Adds or updates tax registration for a single account. You can't set a TRN if there's a pending TRN. You'll need to delete the pending TRN first." Expected: while the addressed account's stored registration is in status `Pending`, a further PutTaxRegistration for that account is refused with the modeled `ConflictException` (HTTP 409) and the stored record is left untouched — the caller must call DeleteTaxRegistration first.

Actual: the handler never looks at the existing record's status. `crate::state::ComAmazonawsTaxsettingsTaxsettingsState::store_registration` consults the previous record only to read its revision number and to decide whether the write would ADD a record past the emulator's resident bound; any existing record (Pending included) is unconditionally replaced, a brand-new validation window is started, and 200 + `{"status":"Pending"}` is returned. The emulator stores every fresh write as `Pending` for `TRN_VALIDATION_DELAY` (500 ms), so the entire documented conflict window exists in this emulator and is simply never enforced — the modeled `ConflictException` is reachable only for the emulator's own resident-store bound, never for the reason AWS documents.

## Evidence

Spec: model documentation on `com.amazonaws.taxsettings#PutTaxRegistration` ("You can't set a TRN if there's a pending TRN. You'll need to delete the pending TRN first."), and the operation declares `ConflictException` (HTTP 409, doc: "The exception when the input is creating conflict with the given state").

Handler: `src/ops/put_tax_registration.rs` `handle()` — calls `check_tax_registration_entry(entry)` then `store_registration(&account_id, entry, now)`; no status check. `src/state.rs` `store_registration()` (approx. lines 258-276) returns `Err` only from `registration_store_full()` when `previous_revision.is_none() && registrations.len() >= MAX_STORED_REGISTRATIONS`; otherwise it bumps the revision and `registrations.insert(...)`, so a Pending record is replaced.

Tests: `tests/all/fidelity_rules.rs::a_submitted_trn_is_pending_until_its_validation_is_due_then_settles` rewrites the registration only AFTER `make_validations_due(&engine)` (i.e. once the record has settled), so no test covers (or asserts) the rewrite-while-Pending case. `grep -i "pending TRN"` over `src/` finds the rule only in the generated doc comments.

## Steps to Reproduce

1. POST /PutTaxRegistration with {"accountId":"000000000001","taxRegistrationEntry":{"registrationId":"DE123456789","registrationType":"VAT","legalName":"Acme GmbH","legalAddress":{"postalCode":"10115","countryCode":"DE"}}} — observe 200 with status "Pending".
2. Immediately (within the 500 ms validation window, before the record settles) POST /PutTaxRegistration again for the same accountId with any valid entry.
3. Observe 200 with status "Pending" and the record replaced (revision bumped, previous document access tokens invalidated). AWS refuses this second call with HTTP 409 ConflictException because a pending TRN exists and must be deleted first.
4. POST /GetTaxRegistration for the account and note the registration reflects the second (should-have-been-refused) write.
