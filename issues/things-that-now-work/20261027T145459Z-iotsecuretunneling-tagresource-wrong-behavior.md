Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# TagResource changes the tunnel's reported tags without stamping the record's lastUpdatedAt

_Filed: 2026-10-27T14:54:59Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` writes into the tunnel's authoritative tag store but never touches the stored tunnel record, so `StoredTunnel::last_updated_at` keeps the value it had before the tagging call. `DescribeTunnel` joins both halves into the one `Tunnel` shape it returns — it reports `tags` from the tag store and `lastUpdatedAt` from the record (describe_tunnel.rs:187-189) — so after a successful `TagResource` a describe reports changed `tags` next to a `lastUpdatedAt` that predates the change, and `ListTunnels` reports the same stale stamp.

This contradicts the rule every other write path of this crate follows: `OpenTunnel` stamps the record it creates, `RotateTunnelAccessToken` stamps the record whose tokens/destinationConfig it changed, and `StoredTunnel::close` — the single writer of the close transition shared by `CloseTunnel` and the expiry sweep — stamps it too. Those stamps are not incidental: the crate's own tests assert them ("rotate must bump lastUpdatedAt", "the close must stamp lastUpdatedAt"). `TagResource` is the only handler that mutates state the `Tunnel` shape reports and leaves the stamp alone, and no comment or test in the crate states a reason for the difference.

The handler already has everything needed to stamp: it resolves the ARN through `state::tunnel_id_for_arn` (tag_resource.rs:152-155) and throws the resolved id away with `.is_none()`, while `state::tunnel_mut(region, tunnel_id)` (state.rs:320-322) is the accessor the sibling mutating handlers use — so this is a skipped existing helper, not new machinery.

## Evidence

Smithy (`smithy_query` shape=Tunnel, member lastUpdatedAt): "<p>The last time the tunnel was updated.</p>" — the field DescribeTunnel reports beside the tags TagResource just changed.

The handler writes tags and returns, with no write to the record:
`services/.../src/ops/tag_resource.rs:197-203`
```
        let stored = store.tags.entry(input.resource_arn.clone()).or_default();
        for tag in &input.tags {
            stored.insert(tag.key.clone(), tag.value.clone());
        }
        drop(store);

        Ok(TagResourceOutput {})
```
It resolved the tunnel id already and discarded it: `services/.../src/ops/tag_resource.rs:152-155` — `if store.tunnel_id_for_arn(&region, &input.resource_arn).is_none() {`.

Sibling write paths in the SAME crate stamp the record:
- `services/.../src/ops/rotate_tunnel_access_token.rs:252` — `tunnel.last_updated_at = now;`
- `services/.../src/state.rs:274-277` (`StoredTunnel::close`, used by `CloseTunnel` and by `materialize_expired_tunnels`) — `self.status = TunnelStatus::Closed;` / `self.last_updated_at = at;`
- `services/.../src/ops/open_tunnel.rs:268-269` — `created_at: now, last_updated_at: now,`

Both halves are reported from one record: `services/.../src/ops/describe_tunnel.rs:187-189`
```
                tags,
                created_at: Some(::aws_smithy_types::DateTime::from(stored.created_at)),
                last_updated_at: Some(::aws_smithy_types::DateTime::from(stored.last_updated_at)),
```

Test coverage of the rule this handler breaks (and none covering tagging):
- `tests/all/tunnel_lifecycle.rs:651-653` — `assert!(after.last_updated_at > before.last_updated_at, "case {mode:?}: rotate must bump lastUpdatedAt");`
- `tests/all/close_tunnel_idempotence.rs:347-352` — `assert!(after.last_updated_at > before.last_updated_at, "case {case:?}: the close must stamp lastUpdatedAt, ...")`
- `grep lastUpdatedAt tests/` shows no assertion for `TagResource`/`UntagResource`.

## Steps to Reproduce

1. Mount the service and call `OpenTunnel` with `{"destinationConfig":{"thingName":"thing-1","services":["SSH"]}}`; note `tunnelId` and `tunnelArn`.
2. Call `DescribeTunnel` with `{"tunnelId":"<id>"}` and record `tunnel.lastUpdatedAt`.
3. Wait a moment, then call `TagResource` with `{"resourceArn":"<arn>","tags":[{"key":"env","value":"test"}]}` → 200.
4. Call `DescribeTunnel` again: `tunnel.tags` now carries `env=test`, but `tunnel.lastUpdatedAt` is unchanged — the record reports an update time older than its last update. `ListTunnels` reports the same stale `lastUpdatedAt`.
5. Contrast with `RotateTunnelAccessToken` (`{"tunnelId":"<id>","clientMode":"ALL"}`) or `CloseTunnel`, which both move `lastUpdatedAt` forward.
