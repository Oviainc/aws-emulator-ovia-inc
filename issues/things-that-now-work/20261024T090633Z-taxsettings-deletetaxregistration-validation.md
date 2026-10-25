Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# DeleteTaxRegistration deletes a Brazil (BR) tax registration instead of refusing the documented Brazil carve-out

_Filed: 2026-10-24T09:06:33Z_
_Operation: DeleteTaxRegistration | Class: validation_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation's own model documentation carries an explicit carve-out: "Deletes tax registration for a single account." followed by the note "This API operation can't be used to delete your tax registration in Brazil. Use the Payment preferences page in the Billing and Cost Management console instead."

Expected: a DeleteTaxRegistration call aimed at an account whose stored registration is a Brazilian one (legalAddress.countryCode == BR, i.e. the CNPJ/CPF registrations the emulator happily stores) is refused by the API — AWS answers the modeled client error (ValidationException, HTTP 400) naming the refusal, and the stored registration is left in place, so a following GetTaxRegistration still reports it.

Actual: the handler removes whatever record the account holds with no regard to the registration's country. `src/ops/delete_tax_registration.rs::handle` resolves the account id and calls `state.lock().registrations.remove(&account_id)`, answering 200 + `{}` for a Brazilian registration exactly as for a German one, and the registration is gone from every later read. The carve-out appears in the crate ONLY as generated documentation text (`src/ops/delete_tax_registration.rs` line 48) — there is no country check anywhere in the delete path.

The state to hit this is reachable today: the emulator stores Brazilian registrations (PutTaxRegistration accepts a `CNPJ`/`CPF` entry whose legalAddress.countryCode is `BR`), so a client that follows a BR write with a delete observes a success AWS does not give.

## Evidence

Spec: `smithy_query(query="doc", shape="DeleteTaxRegistration")` — "<note><p>This API operation can't be used to delete your tax registration in Brazil. Use the Payment preferences page in the Billing and Cost Management console instead.</p></note>". The operation declares ValidationException (HTTP 400) and ConflictException (HTTP 409) as client errors it may answer.

Handler: `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/delete_tax_registration.rs` lines 142-158 — unconditional `registrations.remove(&account_id)`; the only modeled error raised is ResourceNotFoundException for an account that holds nothing. `grep -n 'Brazil|\bBR\b' src/` finds the sentence only inside doc comments (delete_tax_registration.rs:48, batch_delete_tax_registration.rs:56) and never in `src/state.rs` logic.

State reachability: `tests/all/tax_registration_lifecycle.rs` round-trip cases "CNPJ registration for a government sector buyer" (lines 167-183, countryCode "BR") and "CPF registration with a date of birth" (lines 184-195, countryCode "BR") store Brazilian registrations and assert they read back.

Tests: no test exercises deleting a BR registration — `tests/all/tax_registration_lifecycle.rs::deleting_a_registration_removes_it_from_every_read` and the batch-delete tests use DE entries only, so nothing asserts (or contradicts) the carve-out.

## Steps to Reproduce

1. POST /PutTaxRegistration with {"accountId":"000000000003","taxRegistrationEntry":{"registrationId":"12.345.678/0001-95","registrationType":"CNPJ","legalName":"Municipio de Exemplo","sector":"Government","legalAddress":{"districtOrCounty":"Centro","city":"Sao Paulo","stateOrRegion":"SP","postalCode":"01001-000","countryCode":"BR"}}} — observe 200 with status "Pending".
2. POST /DeleteTaxRegistration with {"accountId":"000000000003"}.
3. Observe HTTP 200 with an empty body: the Brazilian registration was deleted. AWS refuses this call (the API cannot delete a Brazilian tax registration; it must be removed from the Billing console) and leaves the record in place.
4. POST /GetTaxRegistration with {"accountId":"000000000003"} — observe 404 ResourceNotFoundException, where AWS would still report the registration.
