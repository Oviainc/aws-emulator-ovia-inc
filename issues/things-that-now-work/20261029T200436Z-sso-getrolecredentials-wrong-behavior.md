Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# An expired role credential keeps being accepted by the emulator unless the same (token, account, role) is asked for again

_Filed: 2026-10-29T20:04:36Z_
_Operation: GetRoleCredentials | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetRoleCredentials` mints a credential in the emulator's credential store, stamps it with an expiration one `ROLE_SESSION_DURATION` out, and reports that instant to the caller as `roleCredentials.expiration`. The handler states the rule it means to hold: a record "past its expiration is spent: forget the credential it names before minting its replacement, so the emulator stops accepting it". But the only code that enforces it — `state.issued_credentials.remove(&key)` followed by `ctx.credential_store().delete(&spent.access_key_id)` — sits inside the re-mint branch, which runs ONLY when a later request presents the SAME `(accessToken, accountId, roleName)` triple. If nobody asks again (the normal case: a CLI signs in, pulls credentials for an account/role once, and stops), the expired credential is never removed from the store and the emulator goes on accepting it for signing indefinitely, long after the `expiration` the response advertised and long after the sign-in session itself is gone. `Logout` does not close the hole either: it deliberately leaves minted credentials alone so they "continue based on the duration configured in the permission set" — but nothing in the crate ever ends them at the end of that duration, so "for the rest of their own duration" is in practice "forever".

Second symptom of the same class on the same handler: the expiration instant is computed with an unguarded `now + crate::state::ROLE_SESSION_DURATION`, while this crate's own `put_session` refuses to do exactly that, with a comment saying why (`SystemTime + Duration` PANICS when the sum is unrepresentable) and using `checked_add` plus a saturating fallback instead. The one arithmetic site that stamps a credential skips the guard the crate wrote for the one that stamps a session.

## Evidence

The rule the handler states, and the only place it is applied — inside the re-mint branch (`services/service-com-amazonaws-sso-swbportalservice/src/ops/get_role_credentials.rs:206-214`):
```rust
        let record = match held {
            Some(record) => record,
            None => {
                // A record that is present but past its expiration is spent: forget the
                // credential it names before minting its replacement, so the emulator
                // stops accepting it.
                if let Some(spent) = state.issued_credentials.remove(&key) {
                    let _ = ctx.credential_store().delete(&spent.access_key_id);
                }
```
That branch is reached only for the same key, because `held` is looked up by that key (`src/ops/get_role_credentials.rs:196-205`):
```rust
        let key = (
            input.access_token.clone(),
            input.account_id.clone(),
            input.role_name.clone(),
        );
        let held = state
            .issued_credentials
            .get(&key)
            .filter(|record| record.is_valid_at(now))
            .cloned();
```
What the crate says the stored record means (`src/state.rs:121-133`):
```rust
/// A role credential `GetRoleCredentials` minted, stored so the same session asking
/// again for the same role sees the same credential until it expires.
///
/// The id and both secrets come from the emulator's credential store, so the
/// credential this service hands back is one the emulator itself will accept.
```
and the expiry predicate that is only ever consulted on that one key (`src/state.rs:135-140`):
```rust
    pub fn is_valid_at(&self, now: SystemTime) -> bool {
        self.expiration_millis > epoch_millis(now)
    }
```
`Logout` is the other path that could end them and explicitly does not (`src/ops/logout.rs:181-185`):
```rust
        // Invalidating the sign-in session is what this operation DOES. The role
        // credentials already minted for it are deliberately left alone: the model's own
        // documentation says role sessions created through a permission set "continue
        // based on the duration configured in the permission set" after logout.
```
So no code path in the crate removes a credential at its expiration; `credential_store().delete` appears exactly once, at `src/ops/get_role_credentials.rs:213`.

Second symptom — the guard the crate wrote for itself (`src/state.rs:221-228`):
```rust
            // `SystemTime + Duration` PANICS on a ttl the platform's clock type cannot
            // represent, and `ttl` is the seeding caller's value, so saturate instead:
            // an unrepresentable expiry is "not expired" for every request anyway.
            expires_at: SystemTime::now()
                .checked_add(ttl)
                .unwrap_or_else(furthest_representable_expiry),
```
and the sibling arithmetic that skips it (`src/ops/get_role_credentials.rs:236-243`):
```rust
                let record = crate::state::RoleCredentialRecord {
                    ...
                    expiration_millis: crate::state::epoch_millis(
                        now + crate::state::ROLE_SESSION_DURATION,
                    ),
                };
```

Test coverage: `tests/all/portal_session.rs:446-591` (`get_role_credentials_mints_stores_and_reuses_a_future_credential`) pins minting, storage, the reuse-inside-the-lifetime path and the 404s, and `tests/all/portal_session.rs:496-502` asserts the expiration is `issue time + ROLE_SESSION_DURATION`. No row advances past that expiration, so neither the "spent record" branch nor the fate of a credential nobody re-requests is pinned anywhere in the suite.

## Steps to Reproduce

1. Seed the portal as `tests/all/portal_session.rs` does: `put_account("111111111111", …, ["AdministratorAccess"])` and `put_session("tok-all", …, ["111111111111"], ttl)`.
2. GET `/federation/credentials?account_id=111111111111&role_name=AdministratorAccess` with `x-amz-sso_bearer_token: tok-all`. Note `roleCredentials.accessKeyId` and `roleCredentials.expiration` from the 200 body.
3. Make the credential due: in the store, rewrite that record's expiry into the past — `portal.state().lock().issued_credentials.get_mut(&("tok-all".into(), "111111111111".into(), "AdministratorAccess".into())).unwrap().expiration_millis = 0;` — which is the same state the clock reaches on its own one `ROLE_SESSION_DURATION` after step 2.
4. Do NOT call `GetRoleCredentials` again for that triple (optionally POST `/logout` with the same token, which the handler documents as leaving credentials alone).
5. Observed: nothing ever calls `ctx.credential_store().delete(&access_key_id)` — the only call site is `src/ops/get_role_credentials.rs:213`, inside the branch that requires a second request for the SAME key — so the access key minted in step 2 is still in the emulator's credential store and still accepted for signing, past the `expiration` the response advertised. Expected per the handler's own comment at `src/ops/get_role_credentials.rs:209-211` ("forget the credential it names … so the emulator stops accepting it") and the expiration the response reported: the credential stops being accepted at its expiration, whether or not anyone asks for that role again.
6. For the second symptom, compare `src/state.rs:221-228` (`checked_add`, with the comment stating that `SystemTime + Duration` panics) against `src/ops/get_role_credentials.rs:240-242` (`now + crate::state::ROLE_SESSION_DURATION`): the same crate guards one expiry computation and not the other.
