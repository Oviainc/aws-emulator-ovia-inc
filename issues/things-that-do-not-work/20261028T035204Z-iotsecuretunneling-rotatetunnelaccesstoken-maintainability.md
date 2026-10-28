Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 1f84dda0f3deaf97a2c7ee51f672a37b240346b3

# RotateTunnelAccessToken writes lastUpdatedAt directly instead of through state::stamp_tunnel_updated

_Filed: 2026-10-28T03:52:04Z_
_Operation: RotateTunnelAccessToken | Class: maintainability_

> _Note: this issue was identified against the code base at commit `1f84dda0f3deaf97a2c7ee51f672a37b240346b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate declares exactly one place the `Tunnel$lastUpdatedAt` stamp is written for a member-level update: `ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState::stamp_tunnel_updated`, whose own doc comment states that EVERY call changing something the tunnel reports goes through it. `TagResource` and `UntagResource` obey that rule and call the helper. `RotateTunnelAccessToken` hand-rolls the same one-line rule (`tunnel.last_updated_at = now;`) on the `&mut StoredTunnel` it already holds, so the stamp rule now exists in two copies in this crate and the two copies can drift (e.g. if the helper later also has to touch an index, a derived field, or the 'did anything change' guard the helper's doc already discusses).

This is purely a duplication of logic: both copies assign the same instant to the same field, so the fix (resolve the mutation, drop the `&mut` borrow, then call `store.stamp_tunnel_updated(&region, &input.tunnel_id, now)`) leaves every response byte-identical and every existing test passing.

## Evidence

The crate's own doc states the rule and names the single site it lives at — `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/state.rs:363-376`:
```
    /// Stamp the tunnel `tunnel_id` names in `region` as last updated at `at`.
    ///
    /// `Tunnel$lastUpdatedAt` is "The last time the tunnel was updated", so
    /// every call that changes something the tunnel reports goes through here
    /// with the instant of its change — whichever member of the record it wrote,
    /// and including the tag set, which `DescribeTunnel` reports inside the same
    /// `Tunnel` shape as this stamp.
    pub fn stamp_tunnel_updated(&mut self, region: &str, tunnel_id: &str, at: SystemTime) {
        if let Some(tunnel) = self.tunnel_mut(region, tunnel_id) {
            tunnel.last_updated_at = at;
        }
    }
```

The second copy of that rule, in the handler that does NOT go through the helper — `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/ops/rotate_tunnel_access_token.rs:252`:
```
        tunnel.last_updated_at = now;
```

Sibling handlers in the SAME crate that DO call the helper:
- `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/ops/tag_resource.rs:208` — `store.stamp_tunnel_updated(&region, &tunnel_id, now);`
- `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/ops/untag_resource.rs:177` — `store.stamp_tunnel_updated(&region, &tunnel_id, now);`

(The third assignment, `src/state.rs:314` inside `StoredTunnel::close`, is the close TRANSITION's own single writer and is shared by `CloseTunnel` and the expiry sweep, so it is not a duplicate of the member-update rule.)

Existing tests that cover both copies and must stay unchanged:
- `tests/all/tunnel_lifecycle.rs:651-653` — `assert!(after.last_updated_at > before.last_updated_at, "case {mode:?}: rotate must bump lastUpdatedAt");`
- `tests/all/tag_write_stamps.rs:5-12` — "RULE: a successful call that changes something a tunnel reports leaves the record's `lastUpdatedAt` ... at the instant of that change, and a call that changes nothing leaves every stamp exactly as it was."

## Steps to Reproduce

1. Open `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/state.rs` and read the doc on `stamp_tunnel_updated` (lines 363-376): it states that every call changing something the tunnel reports goes through that helper.
2. Grep the crate for writes of the stamp: `grep -n 'last_updated_at =\|stamp_tunnel_updated' src/`.
3. Observe `src/ops/tag_resource.rs:208` and `src/ops/untag_resource.rs:177` call the helper, while `src/ops/rotate_tunnel_access_token.rs:252` assigns `tunnel.last_updated_at = now;` itself — the same rule in two places.
4. Run `cargo test -p service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling`: `tests/all/tunnel_lifecycle.rs:651` and `tests/all/tag_write_stamps.rs` pass against either copy, so nothing catches the two drifting apart.
