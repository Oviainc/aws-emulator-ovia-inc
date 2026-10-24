Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 733b41f401fdbdc43a4f3ac4e403542c858679fd

# BatchPutTaxRegistration overwrites accounts holding a Pending TRN instead of reporting a per-account conflict

_Filed: 2026-10-24T04:03:57Z_
_Operation: BatchPutTaxRegistration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `733b41f401fdbdc43a4f3ac4e403542c858679fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

BatchPutTaxRegistration's model documentation carries the same rule as the single-account write: "Adds or updates tax registration for multiple accounts in batch. ... You can't set a TRN if there's a pending TRN. You'll need to delete the pending TRN first." Expected: an account in the batch whose stored registration is still `Pending` is not re-written; it is reported in the response's `errors` list (a `BatchPutTaxRegistrationError` carrying that `accountId` and a conflict `code`/`message`) while the remaining accounts are stored, and that account's existing record is left untouched.

Actual: the per-account loop calls the same `store_registration` the single-account write uses, which replaces any existing record regardless of its status. The only per-account error row this handler can ever emit is the emulator's resident-store bound (`code: "ConflictException"`, message from `registration_store_full()`), so a batch aimed at accounts with outstanding pending TRNs answers an all-success 200 with an empty `errors` list and silently restarts every account's validation.

## Evidence

Spec: model documentation on `com.amazonaws.taxsettings#BatchPutTaxRegistration` (verified via smithy_query doc: "You can't set a TRN if there's a pending TRN. You'll need to delete the pending TRN first."); the operation models `errors` (list<BatchPutTaxRegistrationError>, required) and `ConflictException` (409).

Handler: `src/ops/batch_put_tax_registration.rs` `handle()` — `for account_id in &input.account_ids { match store.store_registration(account_id, entry, written_at) { Ok(status) => ..., Err(conflict) => errors.push(...) } }`; the only `Err` `store_registration` can produce is `registration_store_full()` (`src/state.rs`), never a pending-TRN conflict.

Tests: `tests/all/batch_tax_registration.rs` asserts per-account rows only for the delete path's `ResourceNotFoundException` and for success/overwrite; no test covers a batch write against an account whose TRN is still Pending.

## Steps to Reproduce

1. POST /PutTaxRegistration for accountId "000000000001" with a valid entry — 200, status "Pending".
2. Immediately POST /BatchPutTaxRegistration with {"accountIds":["000000000001","000000000002"],"taxRegistrationEntry":{...valid...}} while the first account's TRN is still Pending.
3. Observe 200 with "errors": [] and the pending account's record replaced. AWS reports that account in the errors list (conflict: a pending TRN must be deleted first) and leaves its record alone, while storing the other account.
