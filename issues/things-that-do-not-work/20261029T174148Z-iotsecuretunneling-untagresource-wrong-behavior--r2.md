Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 42eb24959c6a4eb33ac3ba3d48b702ee83f6032d

# UntagResource samples the clock before taking the store lock, so its lastUpdatedAt stamp can land BEFORE the clock-owed close another request already settled

_Filed: 2026-10-29T17:41:48Z_
_Operation: UntagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `42eb24959c6a4eb33ac3ba3d48b702ee83f6032d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource` reads `SystemTime::now()` into `now` BEFORE it acquires the store lock, and then uses that same pre-lock instant for two different things under the lock: the expiry sweep (`materialize_expired_tunnels(now)`) and the record's own update stamp (`stamp_tunnel_updated(..., now)`).

The comment directly above the sweep states the invariant this ordering is supposed to guarantee:

    // A tunnel whose lifetime has already run out is closed by the clock
    // before this call writes its own stamp, so the stamp this call leaves
    // is never overtaken by a close that was owed earlier.

The code under that comment breaks it whenever the lock is contended. Because `now` is sampled before the lock, another request that wins the lock first can settle the owed close at the tunnel's `expires_at` (the sweep deliberately stamps "the instant the transition came DUE"), and `StoredTunnel::close` is a one-shot — it returns early for a record that is no longer `OPEN`, so the owed stamp is final. This request then acquires the lock, its own `materialize_expired_tunnels(now)` is a no-op (the record is already `CLOSED`), `remove_tags` reports a real change, and `stamp_tunnel_updated` writes `last_updated_at = now`, which is EARLIER than the `closed_at`/`last_updated_at` the owed close just wrote.

The record then reports, through `DescribeTunnel$tunnel.lastUpdatedAt` and `ListTunnels` alike, a "last time the tunnel was updated" that precedes its own close instant — the stamp moved BACKWARDS for a change that happened after the close. That is exactly the state the handler's comment says can never arise, and it also breaks the single-instant shape the crate's own closed-record rule asserts (`tests/all/closed_tunnel_shape.rs`: the record stamp and both ends' stamps are the one instant the close happened at).

`TagResource` carries the identical pre-lock sample and the identical comment (`src/ops/tag_resource.rs:151-157`, `:207-209`), so the same inversion is reachable there; the fix belongs in both, which is why this is described once. The fix is to sample the clock after `state.lock()` so the sweep and the stamp both use an instant no earlier than the one the lock makes visible.

## Evidence

The pre-lock sample and the invariant it is claimed to uphold, `src/ops/untag_resource.rs:138-144`:

        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx);
        let mut store = state.lock();
        // A tunnel whose lifetime has already run out is closed by the clock
        // before this call writes its own stamp, so the stamp this call leaves
        // is never overtaken by a close that was owed earlier.
        store.materialize_expired_tunnels(now);

The same pre-lock `now` used as the update stamp, `src/ops/untag_resource.rs:176-178`:

        if store.remove_tags(&input.resource_arn, input.tag_keys.iter().map(String::as_str)) {
            store.stamp_tunnel_updated(&region, &tunnel_id, now);
        }

The owed close is stamped at the DUE instant, not at the noticing call's clock, `src/state.rs:633-636`:

            if now >= tunnel.expires_at {
                // The stamp is the instant the transition came DUE, not the
                // instant of the call that noticed it.
                tunnel.close(tunnel.expires_at);

and it is a one-shot, so a later caller cannot re-settle it, `src/state.rs:324-330`:

    pub fn close(&mut self, at: SystemTime) -> bool {
        if self.status != TunnelStatus::Open {
            return false;
        }
        self.status = TunnelStatus::Closed;
        self.last_updated_at = at;
        self.closed_at = Some(at);

The stamp writer this handler reaches, which assigns unconditionally, `src/state.rs:388-392`:

    pub fn stamp_tunnel_updated(&mut self, region: &str, tunnel_id: &str, at: SystemTime) {
        if let Some(tunnel) = self.tunnel_mut(region, tunnel_id) {
            tunnel.last_updated_at = at;
        }
    }

The sibling handler with the identical defect, `src/ops/tag_resource.rs:151-157`:

        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx);
        let mut store = state.lock();
        // A tunnel whose lifetime has already run out is closed by the clock
        // before this call writes its own stamp, so the stamp this call leaves
        // is never overtaken by a close that was owed earlier.
        store.materialize_expired_tunnels(now);

Test coverage: `tests/all/tag_write_stamps.rs` and `tests/all/owed_close_settled_by_every_entry_point.rs` drive every tagging entry point one request at a time (each row awaits its `call(...)` before the next), and `tests/all/closed_tunnel_shape.rs` likewise, so no existing test has two requests contending for `state.lock()` — the window between the `SystemTime::now()` on line 138 and the `state.lock()` on line 140 is entirely uncovered.

## Steps to Reproduce

1. Mount the service on a shared `engine::InMemoryEngine` behind `MultiServiceRouter`, as every suite in `tests/all/` does.
2. Call `OpenTunnel` with `{"destinationConfig":{"thingName":"thing-1","services":["SSH"]},"timeoutConfig":{"maxLifetimeTimeoutMinutes":1},"tags":[{"key":"env","value":"test"}]}` and note `tunnelId`/`tunnelArn`.
3. Rewind the stored record's timeline so its `expires_at` is an instant T a hair in the future, as `rewind()` in `tests/all/closed_tunnel_shape.rs` does (adjusting `created_at`, `last_updated_at`, `expires_at` and calling `note_open_expiry`).
4. Dispatch `UntagResource` with `{"resourceArn":"<arn>","tagKeys":["env"]}` so its `SystemTime::now()` on `src/ops/untag_resource.rs:138` is sampled just before T, while another task holds `state.lock()` past T (e.g. hold the guard from `engine.state::<TunnelState>().lock()` in the test while the request is in flight).
5. Release the lock after T, letting any entry point run `materialize_expired_tunnels` first so the owed close is settled with `closed_at = last_updated_at = T` (the instant it came due).
6. Let the `UntagResource` request proceed: its own `materialize_expired_tunnels(now)` is a no-op, `remove_tags` returns `true`, and `stamp_tunnel_updated(..., now)` writes `last_updated_at = now < T`.
7. Call `DescribeTunnel` with `{"tunnelId":"<id>"}`: `tunnel.lastUpdatedAt` is now EARLIER than the close instant the record was closed at, and earlier than the value the call observed before it — the stamp the handler's own comment says "is never overtaken by a close that was owed earlier".
