Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8d70a74790407ac6eb1b5a0a8e857d91d7e5b7f1

# The CLOSED-tunnel transition is implemented twice: CloseTunnel hand-rolls what materialize_expired_tunnels already does

_Filed: 2026-10-25T21:37:28Z_
_Operation: CloseTunnel | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8d70a74790407ac6eb1b5a0a8e857d91d7e5b7f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Exactly one rule describes what it means for a stored tunnel to stop being OPEN in this crate: set `status = TunnelStatus::Closed`, put BOTH connection states to `StoredConnectionState::disconnected(<instant>)`, and stamp `last_updated_at = <instant>`. That four-field rule is written out twice, in two different modules:

1. `src/ops/close_tunnel.rs` for the explicit `CloseTunnel` (with `<instant>` = `now`), and
2. `src/state.rs::materialize_expired_tunnels` for the close the clock owes once `maxLifetimeTimeoutMinutes` runs out (with `<instant>` = `tunnel.expires_at`).

The only difference between the copies is which instant they stamp; everything else is identical. The copies can drift: a future change to what closing a tunnel means (for example also clearing the stored access tokens, or recording a close reason) applied to one copy would leave the other producing a differently-shaped CLOSED record, and `DescribeTunnel`/`ListTunnels` would then report a tunnel whose closed state depends on *how* it closed rather than on the rule. The crate already keeps the "one rule, one place" pattern everywhere else in this module (`stored_destination_config`, `merged_tag_count`, `first_reserved_tag_key`, `tunnel_not_found_message` are each documented as "the single place" a rule lives), so this is the one transition that escaped it.

The defect is the duplication only. The fix is to express the transition once — e.g. an inherent `StoredTunnel` method taking the instant to stamp — and have both sites call it. Every response and every existing test stays byte-for-byte unchanged, including the two different timestamps (explicit close stamps `now`; the expiry sweep stamps `expires_at`).

## Evidence

Copy 1 — `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/ops/close_tunnel.rs:156-163`:

```rust
        } else if let Some(tunnel) = store.tunnel_mut(&region, &input.tunnel_id) {
            tunnel.status = crate::state::TunnelStatus::Closed;
            // "we close the WebSocket connections between the client and proxy
            // server": neither end is connected to a closed tunnel.
            tunnel.source_connection_state = crate::state::StoredConnectionState::disconnected(now);
            tunnel.destination_connection_state =
                crate::state::StoredConnectionState::disconnected(now);
            tunnel.last_updated_at = now;
        }
```

Copy 2 — `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/state.rs:424-430`:

```rust
            if now >= tunnel.expires_at {
                tunnel.status = TunnelStatus::Closed;
                tunnel.last_updated_at = tunnel.expires_at;
                tunnel.source_connection_state =
                    StoredConnectionState::disconnected(tunnel.expires_at);
                tunnel.destination_connection_state =
                    StoredConnectionState::disconnected(tunnel.expires_at);
            } else if earliest.is_none_or(|soonest| tunnel.expires_at < soonest) {
```

That both sites are the SAME rule is stated in the crate's own words: `src/ops/close_tunnel.rs:5-6` — "//! Moves the tunnel to `CLOSED` and disconnects both ends" — and `src/state.rs:400-406` — "/// Close every tunnel whose `maxLifetimeTimeoutMinutes` has run out. /// ... It is /// idempotent, and it stamps `lastUpdatedAt` with the instant the transition /// came due rather than with the time of the call that noticed it."

Test coverage that pins both copies' observable results (so a single-helper refactor is verifiable without changing any assertion):
- `tests/all/tunnel_refusal_rules.rs:195-203` crosses every way a tunnel stops being OPEN — `HowItClosed::CloseDeleteOmitted`, `HowItClosed::CloseDeleteFalse` (copy 1) and `HowItClosed::LifetimeElapsed { minutes: 1 | 60 | 720 }` (copy 2) — and `tests/all/tunnel_refusal_rules.rs:179-184` asserts `described["tunnel"]["status"] == Some("CLOSED")` for each.
- `tests/all/tunnel_lifecycle.rs:732-742` asserts `Some("CLOSED")` plus `last_updated_at >= created_at` after the explicit close (copy 1).
- `tests/all/tunnel_lifecycle.rs:1049-1064` asserts `Some("CLOSED")` and `stored_last_updated == Some(expires_at)` after the lifetime elapses (copy 2), and `tests/all/tunnel_lifecycle.rs:1077-1081` that `ListTunnels` agrees.
- `tests/all/tunnel_lifecycle.rs:309-320` asserts both `sourceConnectionState`/`destinationConnectionState` are `DISCONNECTED` with a set `lastUpdatedAt`.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/ops/close_tunnel.rs` and read lines 156-163: the explicit close sets `status = Closed`, both connection states to `disconnected(now)`, and `last_updated_at = now`.
2. Open `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/state.rs` and read lines 424-430: the expiry sweep sets the same four fields, with `tunnel.expires_at` as the instant.
3. Observe the two blocks differ only in the instant stamped — there is no shared helper either one calls, so the meaning of "this tunnel is CLOSED" is maintained in two modules at once.
4. Confirm the intended fix is behaviour-preserving: `cargo test -p service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling` exercises both paths (`tests/all/tunnel_refusal_rules.rs::rotate_is_refused_once_the_tunnel_is_no_longer_open`, `tests/all/tunnel_lifecycle.rs` close and lifetime-elapsed cases) and must keep passing unchanged after the two copies are collapsed into one helper taking the instant to stamp.
