Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ba10b2e9c791e1406ed6cd1db28496a7b9239da3

# The "address already in use" conflict counts the target account's own current primary email, refusing an update to the address that account already holds

_Filed: 2026-10-24T23:52:58Z_
_Operation: AcceptPrimaryEmailUpdate | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ba10b2e9c791e1406ed6cd1db28496a7b9239da3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AcceptPrimaryEmailUpdate` refuses an acceptance with `ConflictException` when `store.primary_email_in_use(&input.primary_email)` is true. The handler's comment scopes that question to OTHER accounts ("an address another account holds, or has an accepted update for, cannot become this one's"), but the helper iterates every account in the store — the target included — and returns true as soon as `account.primary_email == address`.

So accepting an update for the address the TARGET account already holds is refused with 409 and the message "the email address is already in use as the primary email address of an account" — where that account is the very account being updated, and nothing is being taken from anybody. Concretely, a client that re-runs the start→accept flow for the address it just completed (a natural retry, and the status of the previous update is `COMPLETED`, not `PENDING`, so the acceptance is otherwise valid) gets a conflict instead of completing, and so does an update that re-affirms the account's seeded `root+<id>@example.com`. The helper's own doc states the rule asymmetrically — "either an account holds it, or ANOTHER account's update is ACCEPTED for it" — which is where the target account leaks into the first half of the test.

## Evidence

Handler, services/.../src/ops/accept_primary_email_update.rs:198-203:
```
// Whether the presented address is already in use is read BEFORE the
// target record is borrowed mutably, and it is a question about the
// whole store: an address another account holds, or has an accepted
// update for, cannot become this one's. The target's own update is
// still PENDING at this point, so it never conflicts with itself.
let address_in_use = store.primary_email_in_use(&input.primary_email);
```
and the refusal at accept_primary_email_update.rs:246-253 (`primary_email_already_in_use()`).

Helper, src/state.rs:1063-1080:
```
/// Whether `address` is already in use as an account's primary email
/// address: either an account holds it, or another account's update is
/// ACCEPTED for it, so its completion is already owed.
...
pub fn primary_email_in_use(&self, address: &str) -> bool {
    self.accounts.values().any(|account| {
        account.primary_email == address
```
There is no `account.account_id != target` guard anywhere on that path, so the target account satisfies the first disjunct for its own address.

The modelled conflict this implements, src/state.rs:1216-1219: "the conflict this service's own `ConflictException` documentation gives as its example: 'you try to change an account's root user email to an email address which is already in use'" — an address the account already holds is not a change at all.

Tests: tests/all/primary_email_update_acceptance.rs:652 `accepting_an_address_already_in_use_conflicts` only exercises TWO distinct accounts (MEMBER_ACCOUNTS[0] then [1], etc.), so the self-conflict is uncovered.

## Steps to Reproduce

1. POST /startPrimaryEmailUpdate {"AccountId":"123456789012","PrimaryEmail":"ada@example.com"} → 200 PENDING.
2. POST /acceptPrimaryEmailUpdate {"AccountId":"123456789012","PrimaryEmail":"ada@example.com","Otp":"a1B2c3"} → 200 ACCEPTED; wait for it to settle (GetPrimaryEmail now reports ada@example.com).
3. Re-run the same flow for the same account and the same address: POST /startPrimaryEmailUpdate {"AccountId":"123456789012","PrimaryEmail":"ada@example.com"} → 200 PENDING, then POST /acceptPrimaryEmailUpdate with the same body.
4. Observe 409 ConflictException "the email address is already in use as the primary email address of an account" — the account in question is the target itself, which the handler comment says can never conflict with itself.
