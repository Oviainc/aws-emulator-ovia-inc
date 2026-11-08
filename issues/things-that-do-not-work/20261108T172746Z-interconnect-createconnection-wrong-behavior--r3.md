Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 67c908b9b54a08081e5adb412dff79abde54b844

# A clientToken replay hands back a connection in the terminal `deleted` state as the connection the create produced

_Filed: 2026-11-08T17:27:46Z_
_Operation: CreateConnection | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `67c908b9b54a08081e5adb412dff79abde54b844`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `@idempotencyToken` replay path resolves the token's connection with the plain region-scoped lookup `connection_in`, which deliberately keeps a record readable after `DeleteConnection` has settled it into `deleted`. So a `CreateConnection` retried with the same `clientToken` and the same members AFTER the connection that token created has been deleted is answered HTTP 200 with that deleted record as `connection` — a Connection whose state is `deleted`, whose activation key names nothing acceptable any more, and which the rest of this crate holds is not a resource of the account at all.

The same handler states the opposite rule about terminal records two screens further down, where the quota deliberately does not count them because "a quota is about live resources", and the crate's other resolution paths filter them out through `exists_as_resource` / `is_deleted_or_deleting` with the words "a deleted resource is no longer one this account holds". The replay path is the only resolution in the crate that hands a terminal record back as a live resource, and it also makes the answer to one and the same well-formed request depend on the clock: while the deleted record is inside its retention the retry replays it, and once the record is evicted the index entry is dropped by `drop_dangling_index_entries`, so the very same retry then allocates a brand-new connection. The sibling acceptance handler states the principle this breaks explicitly: "the answer to one and the same well-formed request does not depend on how long the caller took to make it".

The same hole answers a retry made while the connection is still `deleting`.

## Evidence

Replay resolves the record with no terminal-state filter — services/service-com-amazonaws-interconnect-interconnect/src/ops/create_connection.rs:270-272:
```
            if let Some(stored) = store.connection_in(region, &created.connection_id) {
                return Ok(StoreVerdict::Replay(stored.connection.clone()));
            }
```
(`connection_in` filters by Region only — src/state.rs:449-453: `self.connections.get(identifier).filter(|stored| visible_in(stored, region))`.)

The SAME handler, on the same store, treats a `deleted` record as not a live connection — src/ops/create_connection.rs:461-470:
```
        // The account's connection quota, which counts the connections the account
        // HOLDS: a quota is about live resources, so a record already in the
        // terminal `deleted` state does not consume it, even though the store keeps
        // it readable until its retention elapses.
        let live_connections = store
            .connections
            .values()
            .filter(|stored| stored.connection.state != "deleted")
            .count();
```

Every other resolution in the crate filters the terminal record out — src/state.rs:1552-1557:
```
/// Whether `stored` is still a resource of this account: a connection that has reached
/// the terminal `deleted` state is gone, while the record itself stays readable by its
/// identifier so a caller polling a delete sees that terminal state.
fn exists_as_resource(stored: &StoredConnection) -> bool {
    stored.connection.state != DELETED_CONNECTION_STATE
}
```
used by src/state.rs:558-561 (`connection_in_for_arn` … `.filter(|stored| exists_as_resource(stored))`) and mirrored by src/state.rs:467-472 (`proposal_in` … `.filter(|stored| !is_deleted_or_deleting(&stored.connection.state))`).

The clock-independence principle this breaks is stated by the sibling handler — src/ops/accept_connection_proposal.rs:418-423: "The acceptance is this operation's to make, and no amount of elapsed time takes it away: … and the answer to one and the same well-formed request does not depend on how long the caller took to make it."

Test coverage: tests/all/create_connection_client_token_replay.rs states the replay RULE (lines 5-10: "a `@idempotencyToken` replay is answered with the connection the token created only when the repeat IS that request") and exercises replay/mismatch rows only (line 232 `let replay = emulator.ic_ok("CreateConnection", base.clone()).await;`). Nothing in the suite deletes a connection and then retries its `clientToken`, so this path is untested — a grep of that file for `Delete`/`deleted` matches nothing.

## Steps to Reproduce

1. Boot the emulator; call ListEnvironments and pick an environmentId plus one of its available bandwidths, and create a ready Direct Connect gateway to attach to.
2. CreateConnection(environmentId=…, bandwidth=…, attachPoint={directConnectGateway: …}, clientToken="T") -> note connection.id and connection.state == "requested".
3. DeleteConnection(identifier=<that id>), then poll GetConnection until it reports state == "deleted" (the clock settles it after PROVISIONING_SPAN).
4. Repeat the exact CreateConnection request from step 2, same clientToken "T" and same members -> observe HTTP 200 whose `connection` is the deleted record, state == "deleted": a create answered with a connection that does not exist.
5. Wait out DELETED_RECORD_RETENTION (state.rs:85) and repeat the same request again -> now a brand-new connection is allocated, so the answer to one identical request depends only on how long the caller waited.
