Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 16003bcc8481d509912583747e009ee504542034

# CloseTunnel re-closes an already-CLOSED tunnel, destroying the expiry-owed lastUpdatedAt stamp its own sweep just wrote

_Filed: 2026-10-27T11:54:30Z_
_Operation: CloseTunnel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `16003bcc8481d509912583747e009ee504542034`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CloseTunnel`'s handler first settles the close the clock already owed (`store.materialize_expired_tunnels(now)`), which the state module documents as idempotent and as stamping `lastUpdatedAt` with the instant the transition came DUE rather than with the time of the call that noticed it. The very next statements in the same function then apply the close transition again, unconditionally and WITHOUT checking whether the record is already `CLOSED`: they re-set `status = Closed`, overwrite BOTH connection states with `disconnected(now)`, and overwrite `last_updated_at = now`.

So in a single `CloseTunnel` invocation on a tunnel whose `maxLifetimeTimeoutMinutes` has already elapsed, line 141 writes the documented `expires_at` stamp and lines 156-163 immediately replace it with `now` — the time of the call that noticed it, which is exactly what the crate's own rule says the stamp must not be. The same unconditional re-close also makes a second `CloseTunnel` on an already-explicitly-closed tunnel non-idempotent: nothing about the tunnel's state changes, yet `tunnel.lastUpdatedAt`, `tunnel.sourceConnectionState.lastUpdatedAt` and `tunnel.destinationConnectionState.lastUpdatedAt` all move forward, and a later `DescribeTunnel`/`ListTunnels` reports the moved timestamps. The sibling writer of the same transition (`materialize_expired_tunnels`) is explicitly idempotent; this one is not, and the fix is to apply the transition only when the resolved record is still `OPEN`.

## Evidence

The rule, in the crate's own words — `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/state.rs:400-406`:

```rust
    /// Close every tunnel whose `maxLifetimeTimeoutMinutes` has run out.
    ///
    /// This is the one write a reporting operation may make: the transition was
    /// already owed at `now` by the clock ... It is
    /// idempotent, and it stamps `lastUpdatedAt` with the instant the transition
    /// came due rather than with the time of the call that noticed it.
```

and the sweep that implements it — `src/state.rs:424-430`:

```rust
            if now >= tunnel.expires_at {
                tunnel.status = TunnelStatus::Closed;
                tunnel.last_updated_at = tunnel.expires_at;
                tunnel.source_connection_state =
                    StoredConnectionState::disconnected(tunnel.expires_at);
```

The code that breaks it, in ONE handler body — `src/ops/close_tunnel.rs:140-164`:

```rust
        let mut store = state.lock();
        store.materialize_expired_tunnels(now);          // line 141: stamps expires_at
        ...
        if input.delete.unwrap_or(false) {
            store.remove_tunnel(&region, &input.tunnel_id);
        } else if let Some(tunnel) = store.tunnel_mut(&region, &input.tunnel_id) {
            tunnel.status = crate::state::TunnelStatus::Closed;   // line 157: no status check
            tunnel.source_connection_state = crate::state::StoredConnectionState::disconnected(now);
            tunnel.destination_connection_state =
                crate::state::StoredConnectionState::disconnected(now);
            tunnel.last_updated_at = now;                        // line 163: overwrites expires_at
        }
```

There is no `if tunnel.status == TunnelStatus::Open` guard anywhere between lines 143 and 164, so the branch runs for a record the sweep on line 141 has just closed.

Test coverage: `tests/all/tunnel_lifecycle.rs:1049-1064` pins the documented stamp on the sweep path only (`assert_eq!(stored_last_updated, Some(expires_at), "case {minutes}: lastUpdatedAt must be the instant the close came due")`), reached through `DescribeTunnel`. `tests/all/tunnel_lifecycle.rs:732-742` asserts only `last_updated_at >= created_at` after an explicit close, and `tests/all/rotate_refusal_error_contract.rs:197-241` never calls `CloseTunnel` on an already-closed record — so no test exercises `CloseTunnel` after the lifetime elapsed, which is why the overwrite is unnoticed.

## Steps to Reproduce

1. Mount the service and call `OpenTunnel` with `{"destinationConfig":{"thingName":"thing-1","services":["SSH"]},"timeoutConfig":{"maxLifetimeTimeoutMinutes":1}}`; note the `tunnelId`.
2. Move the record's timeline into the past so its lifetime has run out (as `tests/all/rotate_refusal_error_contract.rs:213-227` does: shift `created_at`/`last_updated_at`/`expires_at` back and call `note_open_expiry`).
3. Call `DescribeTunnel` with `{"tunnelId":"<id>"}`: the sweep has settled the owed close, `tunnel.status == "CLOSED"`, and `tunnel.lastUpdatedAt` is the instant the close came due (`expires_at`), per `src/state.rs:405-406`.
4. Call `CloseTunnel` with `{"tunnelId":"<id>"}` (no `delete`) — the record is already `CLOSED`, so nothing about its state changes; the call answers 200.
5. Call `DescribeTunnel` again: `tunnel.lastUpdatedAt` is now the instant of the `CloseTunnel` call, and `sourceConnectionState.lastUpdatedAt` / `destinationConnectionState.lastUpdatedAt` have moved with it. The documented `expires_at` stamp is gone, overwritten by the time of the call that merely noticed the already-settled close.
6. Repeat `CloseTunnel` on the same already-closed tunnel: each call moves the three timestamps again, so the operation is not idempotent, unlike the sweep that implements the same transition.
