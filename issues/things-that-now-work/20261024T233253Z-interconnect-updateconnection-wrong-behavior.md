Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# A retried UpdateConnection bandwidth change is refused with 400 instead of being the no-op the handler claims

_Filed: 2026-10-24T23:32:53Z_
_Operation: UpdateConnection | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateConnection` is modelled `@idempotent` with a `clientToken` (`@idempotencyToken`), and the handler explicitly declines to keep a replay index on the grounds that the update is "already idempotent by construction" and that "re-applying the same request is a no-op". The code under that comment contradicts it.

A bandwidth change sets the stored connection's state to `updating` and arms a pending transition due `PROVISIONING_SPAN` (400 ms) later. The retry of that very same request then reaches the permitted-state guard EARLIER in the same handler, which only admits `requested` and `available`; `updating` is not in that allowlist, so the retry is refused with `InterconnectClientException` (HTTP 400) and never reaches the bandwidth comparison the comment relies on. The claimed no-op path is unreachable for exactly the request that produced the `updating` state, so a client (or SDK) that retries a bandwidth update inside the provisioning window gets an error for a request that already succeeded.

The crate's sibling mutators both handle their own retry: `DeleteConnection` branches on the states its own call produces so a repeat "neither restarts the transition nor fails", and `CreateConnection` keeps an `idempotency_tokens` replay index. `UpdateConnection` is the only mutator whose retry handling is asserted in a comment but absent from the code, and `input.client_token` is read by nothing in the handler.

## Evidence

services/service-com-amazonaws-interconnect-interconnect/src/ops/update_connection.rs:220-225 (the rule the code is supposed to obey):
```
// `clientToken` needs no replay index of its own: this update is already
// idempotent by construction. Each supplied member is assigned its
// requested value, so re-applying the same request is a no-op — the
// bandwidth comparison below sees the value it already wrote and neither
// re-arms the `updating` transition nor touches the record again.
```
Same file, lines 180-187 — the guard that runs FIRST and makes that unreachable on a retry:
```
if !crate::state::is_updatable_state(&stored.connection.state) {
    return Err(UpdateConnectionErrorModeled::InterconnectClientException(
        InterconnectClientException {
            message: crate::state::connection_not_updatable_message(&input.identifier),
        },
    )
    .into());
}
```
Same file, lines 229-240 — the first call is what put the record outside that allowlist:
```
if bandwidth != stored.connection.bandwidth {
    stored.connection.bandwidth = bandwidth.clone();
    stored.connection.state = "updating".to_string();
    stored.pending_transition = Some(crate::state::PendingTransition {
        due: now + crate::state::PROVISIONING_SPAN, state: settled });
}
```
src/state.rs:612 `pub const UPDATABLE_CONNECTION_STATES: [&str; 2] = ["requested", "available"];` — `updating` is excluded, and state.rs:610 names it among the states an update is refused on ("`pending`, `down`, `deleting`, `deleted`, `failed`, `updating`").

Sibling that does implement retry tolerance — src/ops/delete_connection.rs:163-173:
```
// @idempotent: a connection already in one of those two states is left
// exactly as it is, and the same record is returned — a retried delete
// neither restarts the transition nor fails.
if stored.connection.state != "deleting" && stored.connection.state != "deleted" {
```
and src/ops/create_connection.rs:177-206, which replays a repeated `clientToken` from `store.idempotency_tokens`.

Model (smithy_query, operation=UpdateConnection): `"clientToken": {"target": "String", "idempotencyToken": true}`, `"idempotent": true`.

Test coverage: none pins this. `grep clientToken services/service-com-amazonaws-interconnect-interconnect/tests` matches only tests/all/connection_lifecycle.rs:659,670 (CreateConnection). tests/all/connection_lifecycle.rs:1097-1256 (`update_connection_changes_only_the_members_it_is_given`) issues each update once, and tests/all/refusals_and_gateway_attach_points.rs:312-340 only retries with a DIFFERENT request (a description change) and accepts either 200 or 400 for it — no case repeats the identical bandwidth request.

## Steps to Reproduce

1. Boot the emulator, ListEnvironments and pick an environment with two available bandwidths (e.g. the Azure seed: 1Gbps, 10Gbps).
2. CreateConnection on it with bandwidth 1Gbps; note the returned `id` (state `requested`).
3. UpdateConnection {identifier: id, bandwidth: "10Gbps", clientToken: "t1"} -> HTTP 200, connection reported in state `updating`.
4. Immediately (inside the 400 ms PROVISIONING_SPAN) send the byte-identical request again, as an SDK retry would: UpdateConnection {identifier: id, bandwidth: "10Gbps", clientToken: "t1"}.
5. Observe HTTP 400 InterconnectClientException "Unable to process request: Cannot update connection <id>: only connections in 'requested' or 'available' state can be updated" — not the no-op replay the handler comment at update_connection.rs:220-225 asserts, and not what DeleteConnection does for its own repeat.
