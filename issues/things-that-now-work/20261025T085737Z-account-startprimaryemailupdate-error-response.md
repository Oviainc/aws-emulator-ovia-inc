Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: dd185e64193b12242704f82dcc412d70cd33b9eb

# StartPrimaryEmailUpdate records a PENDING update for an address another account already holds, never answering the modelled ConflictException

_Filed: 2026-10-25T08:57:37Z_
_Operation: StartPrimaryEmailUpdate | Class: error-response_

> _Note: this issue was identified against the code base at commit `dd185e64193b12242704f82dcc412d70cd33b9eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate owns a store-wide helper that answers whether a primary email address is already in use, and `AcceptPrimaryEmailUpdate` calls it and refuses with the modelled 409 `ConflictException` before confirming an update. `StartPrimaryEmailUpdate` — the operation that starts the root-user email change and that declares `ConflictException` among its modelled errors — never calls that helper and constructs no `ConflictException` anywhere in the handler: it answers 200 with `Status: PENDING` and persists a `PrimaryEmailUpdate` for an address another account already holds as its `primary_email`. The caller is told the update was started, and only discovers at `AcceptPrimaryEmailUpdate` time (after the one-time passcode round trip) that the change can never be applied. The service's own `ConflictException` documentation gives this exact situation as its example, and the handler for the sibling step of the very same flow already implements the check with the existing helper, so this is the one path in the pair that skips a helper the crate already has.

## Evidence

The model's own documented trigger for this error (smithy_query(query="doc",shape="ConflictException")): "The request could not be processed because of a conflict in the current status of the resource. For example, this happens if you try to enable a Region that is currently being disabled (in a status of DISABLING) or if you try to change an account’s root user email to an email address which is already in use."

The error IS modelled for this operation, services/service-com-amazonaws-account-account/src/ops/start_primary_email_update.rs:105:
```
    ConflictException(ConflictException),
```
and `grep -n ConflictException start_primary_email_update.rs` returns only that declaration and the doc header at line 74 — the handler never constructs one.

The helper the crate already has, services/service-com-amazonaws-account-account/src/state.rs:1065-1082:
```
    /// Whether `address` is already in use as an account's primary email
    /// address: either an account holds it, or another account's update is
    /// ACCEPTED for it, so its completion is already owed.
    pub fn primary_email_in_use(&self, address: &str) -> bool {
```
and its stated purpose on the stored field, state.rs:816-817: "and read to decide whether an address is already in use — which is the conflict Account Management documents for a root user email change."

The sibling step that DOES call it, services/service-com-amazonaws-account-account/src/ops/accept_primary_email_update.rs:203 and :246-253:
```
        let address_in_use = store.primary_email_in_use(&input.primary_email);
...
        if address_in_use {
            // The conflict this service's `ConflictException` documents by
            // example: a root user email cannot be changed to an address that
            // is already in use.
            return Err(AcceptPrimaryEmailUpdateErrorModeled::ConflictException(
                crate::state::primary_email_already_in_use(),
```

What the start path does instead, start_primary_email_update.rs:205-215 — settle, then store the request unconditionally:
```
        account.settle_primary_email_update(now);
        let update = crate::state::PrimaryEmailUpdate::started(input.primary_email, now);
        let status = update.status.clone();
        account.primary_email_update = Some(update);

        Ok(StartPrimaryEmailUpdateOutput {
            status: Some(status),
        })
```

Test coverage: `grep -n 'in use|Conflict|conflict' tests/all/primary_email_update.rs` → "No matches" — the start suite (tests/all/primary_email_update.rs) exercises only the own-account refusal and the PENDING status, so no test pins the 200 this issue reports and none would have to change in wording.

## Steps to Reproduce

1. Start and complete an update so a member account holds a known address, or simply read an existing account's primary email: POST /getPrimaryEmail with {"AccountId":"111111111111"} → 200 with PrimaryEmail root+111111111111@example.com.
2. POST /startPrimaryEmailUpdate with {"AccountId":"222222222222","PrimaryEmail":"root+111111111111@example.com"} — an address account 111111111111 already holds as its primary email.
3. Observe 200 with {"Status":"PENDING"}: the request is recorded even though the address is already in use, and no 409 ConflictException is returned.
4. POST /acceptPrimaryEmailUpdate with {"AccountId":"222222222222","PrimaryEmail":"root+111111111111@example.com","Otp":"a1B2c3"} → 409 ConflictException "...already in use as the primary email address of an account", i.e. the conflict the start call could already have reported with the helper it skipped.
