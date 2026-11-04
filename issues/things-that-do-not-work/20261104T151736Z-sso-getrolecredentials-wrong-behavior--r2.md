Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6bd8c1744e5e608eb55402778fdb36afe82c1bfc

# A minted role credential keeps signing requests past the expiration it advertised whenever no further portal call arrives, contradicting the crate's "stops accepting it at exactly the expiration" rule

_Filed: 2026-11-04T15:17:36Z_
_Operation: GetRoleCredentials | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6bd8c1744e5e608eb55402778fdb36afe82c1bfc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetRoleCredentials` mints a session credential in the emulator's credential store and advertises an `expiration` in its response. Both this handler's own module doc and the state module's `end_expired_credentials` doc state the rule that the emulator stops accepting that credential for signing AT that instant. The code implements no such thing: the ONLY code path that ever removes the credential from the emulator's credential store is `crate::state::end_expired_credentials`, and grep shows it is called from exactly four places — the four portal handlers (`get_role_credentials.rs:171`, `list_accounts.rs:175`, `list_account_roles.rs:167`, `logout.rs:188`). There is no timer, no background sweep, and nothing hooked into signature verification.

So the credential's end is not an event of the clock at all, it is an event of unrelated portal traffic. If a caller fetches a role credential and then makes no further portal request (the normal shape of a client that signs in once and spends the next hours calling other services), the credential record survives in `emulator_server::CredentialStore` indefinitely and the emulator keeps verifying SigV4 requests signed with it: `CredentialStore::secret_for` looks up only by id and scheme, and `CredentialRecord` holds nothing but `{ scheme, secrets }`, so the store itself has no notion of expiry to fall back on. A request that must be refused (the role session is over) is answered as an authenticated request — an error turned into a success, for an unbounded window.

The crate already applies the expiry lazily, at the moment of use, for its OTHER consumer of the same record: `signed_in_subject` filters on `record.is_valid_at(now)`, so an expired credential resolves to no Identity Center subject whether or not any sweep has run. Only the credential-store copy depends on a sweep that an arbitrary request has to trigger. The result is observable and absurd: whether the emulator accepts a given expired credential depends on whether somebody happened to call `ListAccounts`/`Logout` in the meantime.

This is also exactly what the existing test does NOT cover: `tests/all/credential_expiry.rs` makes a credential due, then ALWAYS runs a portal request before asserting, and asserts only on this service's own store (`issued_credentials`, `signed_in_subject`). No test asks whether the emulator still accepts the spent credential for signing, so the "at exactly the expiration" half of the quoted rule is unpinned.

## Evidence

The rule, stated by the handler under review — `services/service-com-amazonaws-sso-swbportalservice/src/ops/get_role_credentials.rs:10-13`:
```
//! expires. At that expiration the credential is ended — in this store and in the
//! emulator's credential store — by [`crate::state::end_expired_credentials`], which
//! every operation of this service runs, so the instant this response advertises is the
//! instant the emulator stops accepting what it minted.
```

The same rule restated on the helper — `src/state.rs:350-358`:
```
/// End every minted role credential the clock has made spent at `now`: drop its record
/// here and forget the credential it names in the emulator's credential store, so the
/// emulator stops accepting it for signing at exactly the `expiration` the response that
/// minted it advertised.
///
/// Every handler calls this before it serves, because the expiration is due at its
/// instant and not at the next request for the same role: a credential nobody asks about
/// again has still run out.
```

The code under that doc, and its only trigger — `src/state.rs:364-378`:
```rust
pub fn end_expired_credentials(
    state: &mut ComAmazonawsSsoSwbportalserviceState,
    credential_store: &::emulator_server::CredentialStore,
    now: SystemTime,
) {
    for access_key_id in state.expire_credentials(now) {
        let _ = credential_store.delete(&access_key_id);
```
called from nowhere but the four handlers (`src/ops/get_role_credentials.rs:171`, `src/ops/list_accounts.rs:175`, `src/ops/list_account_roles.rs:167`, `src/ops/logout.rs:188`) — i.e. only when a portal request happens to arrive, never "at exactly the `expiration`".

The store the claim is made about has no expiry of its own to cover the gap — `vendor/server/src/credential_store.rs:51-56` and `:87-90`:
```rust
    pub(crate) fn secret_for(&self, id: &str, scheme: &str, name: &str) -> Result<String, Denied> {
        let records = self.records.read();
        let record = records.get(id).filter(|record| record.scheme == scheme);
        let secret = record.and_then(|record| record.secrets.get(name));
        secret.cloned().ok_or(ACCESS_DENIED)
    }
...
struct CredentialRecord {
    scheme: &'static str,
    secrets: Secrets,
}
```

The crate's own use-time expiry check, applied to the other consumer of the identical record — `src/state.rs:319-325`:
```rust
    pub fn signed_in_subject(&self, access_key_id: &str, now: SystemTime) -> Option<&str> {
        self.issued_credentials
            .iter()
            .find(|(_, record)| record.access_key_id == access_key_id && record.is_valid_at(now))
```

Test coverage: `tests/all/credential_expiry.rs:235-304` always runs a portal request (`run_next`) before it asserts, and asserts only `issued_credentials` and `signed_in_subject`; its header (`:3-6`) claims "at that instant the portal forgets it — in its own store and in the emulator's credential store, which is what stops the emulator accepting it for signing", but no row omits the intervening portal request and no row attempts to sign anything with the spent key (`grep credential_store|secret_for|signing` over `tests/` matches only that comment).

## Steps to Reproduce

1. Seed the portal as `tests/all/portal_session.rs:85-114` does: `put_account(...)` for an account with a role, then `put_session("tok", "user@example.com", [account_id], ttl)`; register `EmulatedService` on a `MultiServiceRouter`.
2. `GET https://portal.sso.us-east-1.amazonaws.com/federation/credentials?account_id=111111111111&role_name=AdministratorAccess` with `x-amz-sso_bearer_token: tok`. Note `roleCredentials.accessKeyId`, `secretAccessKey` and `expiration`.
3. Put the clock past that `expiration` exactly as `tests/all/credential_expiry.rs:226-233` does (`store.issued_credentials.get_mut(&key).unwrap().expiration_millis = <a past value>`), and make NO further portal request.
4. Sign any request to any emulated service with that `accessKeyId`/`secretAccessKey`. Observed: the signature verifies — `CredentialStore::secret_for` (vendor/server/src/credential_store.rs:51-56) still holds the record, because nothing has run `end_expired_credentials`. Expected per `src/ops/get_role_credentials.rs:12-13` and `src/state.rs:351-353`: the emulator stopped accepting it at the advertised `expiration`.
5. Now issue any unrelated portal call (`GET /assignment/accounts` with `tok`, or even with a token naming no session) and repeat the previous step: the same credential is now refused. Acceptance of a spent role credential therefore depends on unrelated portal traffic, which is the contradiction.
6. Contrast the other consumer of the same record on the same state: `store.signed_in_subject(&access_key_id, SystemTime::now())` already answers `None` immediately after step 3, with no sweep (src/state.rs:319-325).
