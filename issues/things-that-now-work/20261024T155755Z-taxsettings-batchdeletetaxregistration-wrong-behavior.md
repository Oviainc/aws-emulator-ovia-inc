Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# BatchDeleteTaxRegistration reports a spurious ResourceNotFoundException error row for an accountId named twice in one request

_Filed: 2026-10-24T15:57:55Z_
_Operation: BatchDeleteTaxRegistration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `BatchDeleteTaxRegistrationRequest$accountIds` is documented as "List of unique account identifiers", and one request deletes the registration of each account it names. An id that appears twice names the same account and the same single deletion, so the response's `errors` list should not contain an entry for an account whose registration this very call removed successfully.

Actual: the handler loops over `input.account_ids` verbatim with no de-duplication: the first occurrence removes the record and reports nothing, and the second occurrence finds nothing left and pushes a `BatchDeleteTaxRegistrationError{accountId, code: "ResourceNotFoundException", message: "No tax registration found"}` row. The caller is told the deletion of an account failed with "not found" in the same response in which that account's registration was in fact deleted — a self-contradictory result that a client reconciling `errors` against its request will treat as a partial failure and retry or alarm on.

The sibling batch handlers in this same crate already treat a repeated id as one address — `BatchPutTaxRegistration` keeps a `BTreeSet` of `addressed` ids and skips a repeat (`src/ops/batch_put_tax_registration.rs` ~lines 226-235), and `BatchGetTaxExemptions` collapses repeats before reporting (`src/ops/batch_get_tax_exemptions.rs` ~lines 162-167). Only the delete path diverges, so the three batch operations of one service disagree about what a repeated account id means.

## Evidence

Spec: `smithy_query(query="doc", shape="com.amazonaws.taxsettings#BatchDeleteTaxRegistrationRequest", member="accountIds")` returns "List of unique account identifiers."; `smithy_query(query="operation", operation="BatchDeleteTaxRegistration")` shows `accountIds` @length 1..=5 and a required `errors` output list of `BatchDeleteTaxRegistrationError{accountId, code, message}` — a per-account failure channel for accounts whose deletion did not happen, which is not the case for an account this call deleted.

Handler: `src/ops/batch_delete_tax_registration.rs` `handle()` lines ~157-175 — `for account_id in &input.account_ids { if store.registrations.remove(account_id).is_none() { errors.push(... code: "ResourceNotFoundException" ...) } }`, with no `addressed`/dedup set, unlike `src/ops/batch_put_tax_registration.rs:226-235` and `src/ops/batch_get_tax_exemptions.rs:162-167` which both dedupe explicitly and document why.

Tests: `tests/all/batch_tax_registration.rs::a_batch_delete_removes_every_registered_account_it_names` (~line 473) and `::an_account_the_batch_could_not_write_is_reported_against_that_account` (~line 661) exercise distinct ids only; no test sends the same accountId twice to /BatchDeleteTaxRegistration, so the behavior is unasserted.

## Steps to Reproduce

1. POST /PutTaxRegistration for accountId 000000000001 with a complete taxRegistrationEntry; observe 200.
2. POST /BatchDeleteTaxRegistration with {"accountIds":["000000000001","000000000001"]}.
3. Observe 200 whose errors list carries one row {"accountId":"000000000001","code":"ResourceNotFoundException","message":"No tax registration found"} even though that account's registration was deleted by this same call — expected an empty errors list.
4. POST /GetTaxRegistration for 000000000001 and observe 404, confirming the deletion did happen and the reported error row is spurious.
5. Compare with /BatchPutTaxRegistration given {"accountIds":["000000000002","000000000002"],...}, which de-duplicates and reports no error, showing the inconsistency inside the service.
