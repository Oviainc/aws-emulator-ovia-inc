Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# UntagResource removes tags the tunnel reports without stamping the record's lastUpdatedAt

_Filed: 2026-10-27T14:54:59Z_
_Operation: UntagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource` deletes keys from the tunnel's tag store and returns without touching the stored tunnel record, so `StoredTunnel::last_updated_at` keeps its pre-call value. Because `DescribeTunnel` reports the tag store and the record's `lastUpdatedAt` inside the one `Tunnel` shape (describe_tunnel.rs:187-189), a successful untag changes what the tunnel reports while leaving the "last time the tunnel was updated" stamp behind.

This is the same divergence as in `TagResource`, filed separately because it is a second handler: every other mutating path in this crate stamps the record (`RotateTunnelAccessToken`, `StoredTunnel::close` shared by `CloseTunnel` and the expiry sweep, `OpenTunnel`), and the crate's tests assert those stamps. Nothing in the crate states a reason for the tag paths to differ, and the stamping accessor already exists (`state::tunnel_mut`), reachable from the id this handler already resolves via `tunnel_id_for_arn` and discards.

## Evidence

Smithy (`smithy_query` shape=Tunnel, member lastUpdatedAt): "<p>The last time the tunnel was updated.</p>".

Handler removes keys and returns, no write to the record:
`services/.../src/ops/untag_resource.rs:167-174`
```
        if let Some(stored) = store.tags.get_mut(&input.resource_arn) {
            for key in &input.tag_keys {
                stored.remove(key);
            }
        }
        drop(store);

        Ok(UntagResourceOutput {})
```
It resolved the record first and dropped the id: `services/.../src/ops/untag_resource.rs:138-141` — `if store.tunnel_id_for_arn(&region, &input.resource_arn).is_none() {`; the mutating accessor exists at `services/.../src/state.rs:320-322` (`pub fn tunnel_mut`).

Sibling write paths in the SAME crate stamp the record: `services/.../src/ops/rotate_tunnel_access_token.rs:252` — `tunnel.last_updated_at = now;`; `services/.../src/state.rs:274-277` — `self.last_updated_at = at;` inside `StoredTunnel::close`.

DescribeTunnel reports both from one record: `services/.../src/ops/describe_tunnel.rs:187-189` — `tags,` / `last_updated_at: Some(::aws_smithy_types::DateTime::from(stored.last_updated_at)),`.

Tests pin the stamp for the other writers and say nothing about untagging: `tests/all/tunnel_lifecycle.rs:651-653` ("rotate must bump lastUpdatedAt"), `tests/all/close_tunnel_idempotence.rs:347-352` ("the close must stamp lastUpdatedAt").

## Steps to Reproduce

1. Call `OpenTunnel` with `{"tags":[{"key":"env","value":"test"}],"destinationConfig":{"thingName":"thing-1","services":["SSH"]}}`; note `tunnelId` and `tunnelArn`.
2. Call `DescribeTunnel` and record `tunnel.lastUpdatedAt`.
3. Call `UntagResource` with `{"resourceArn":"<arn>","tagKeys":["env"]}` → 200.
4. Call `DescribeTunnel` again: `tunnel.tags` is gone but `tunnel.lastUpdatedAt` is unchanged, so the reported update time is older than the tunnel's last change; `ListTunnels` reports the same stale stamp.
