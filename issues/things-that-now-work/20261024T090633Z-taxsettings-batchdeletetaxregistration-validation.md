Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# BatchDeleteTaxRegistration deletes Brazil (BR) registrations instead of reporting the documented Brazil carve-out per account

_Filed: 2026-10-24T09:06:33Z_
_Operation: BatchDeleteTaxRegistration | Class: validation_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation's model documentation carries the same explicit carve-out as the single-account delete: "Deletes tax registration for multiple accounts in batch. This can be used to delete tax registrations for up to five accounts in one batch." followed by "This API operation can't be used to delete your tax registration in Brazil. Use the Payment preferences page in the Billing and Cost Management console instead."

Expected: an account named in `accountIds` whose stored registration is Brazilian (legalAddress.countryCode == BR) is NOT deleted — AWS refuses that account (a row in the response's `errors` list carrying that `accountId`, which is the per-account error channel this operation models, or a call-level ValidationException), and the registration survives the batch so a later GetTaxRegistration/ListTaxRegistrations still reports it. The rest of the batch proceeds.

Actual: `src/ops/batch_delete_tax_registration.rs::handle` loops over `input.account_ids` and calls `store.registrations.remove(account_id)` for each, pushing an error row ONLY when the account held no record at all. A Brazilian registration is removed and reported as a clean success (that account is absent from `errors`), so a client sees the batch delete a record AWS protects. The carve-out exists in the crate only as generated documentation text (`src/ops/batch_delete_tax_registration.rs` line 56); no country check exists in the delete path or in `src/state.rs`.

## Evidence

Spec: `smithy_query(query="doc", shape="BatchDeleteTaxRegistration")` — "<note><p>This API operation can't be used to delete your tax registration in Brazil...</p></note>"; output member `errors` (BatchDeleteTaxRegistrationErrors, required) is the modeled per-account failure channel, and the operation also declares ValidationException (HTTP 400).

Handler: `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/batch_delete_tax_registration.rs` lines 157-175 — `for account_id in &input.account_ids { if store.registrations.remove(account_id).is_none() { errors.push(... code: "ResourceNotFoundException") } }`. The only judged condition is absence of a record; the record's country is never inspected.

State reachability: BR registrations are storable and stored by `tests/all/tax_registration_lifecycle.rs` round-trip cases (countryCode "BR", registrationType CNPJ/CPF).

Tests: `tests/all/batch_tax_registration.rs::a_batch_delete_removes_every_registered_account_it_names` (lines 472-530) uses the DE VAT entry for every row and asserts only registered/unregistered mixes — no test covers a Brazilian registration in a batch delete.

## Steps to Reproduce

1. POST /PutTaxRegistration for "000000000003" with a Brazilian entry (registrationType "CNPJ", legalAddress.countryCode "BR") and for "000000000001" with a German VAT entry (countryCode "DE"); both answer 200.
2. POST /BatchDeleteTaxRegistration with {"accountIds":["000000000001","000000000003"]}.
3. Observe HTTP 200 with "errors": [] — both registrations were deleted, including the Brazilian one. AWS refuses the Brazilian account (that account must be reported as a failure / left untouched) while deleting the German one.
4. POST /GetTaxRegistration with {"accountId":"000000000003"} — observe 404, where AWS would still report the Brazilian registration.
