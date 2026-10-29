Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5cb591631d698c9098c00693287b08686d77b0d1

# ListTagsForResource is the only handler that does not settle the clock-owed close before reporting

_Filed: 2026-10-28T18:01:46Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5cb591631d698c9098c00693287b08686d77b0d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other handler in this crate takes the store lock and immediately calls `store.materialize_expired_tunnels(now)` — the crate's single, shared "settle the transition the clock already made due" step — before it reads or writes anything. `ListTagsForResource` is the one handler that skips it: it takes a non-mutable `state.lock()` guard and goes straight to resolving the ARN and projecting the tag list, so it serves its answer out of a store in which a tunnel whose `maxLifetimeTimeoutMinutes` has already run out is still recorded as `OPEN`, leaving the owed transition for whichever call happens next.

The skipped step is the one the crate documents as mandatory for a reporting operation (state.rs: "This is the one write a reporting operation may make: the transition was already owed at `now` ... whether or not anybody called"), and the two sibling ARN-addressed tag handlers (`TagResource`, `UntagResource`) as well as the two sibling readers (`DescribeTunnel`, `ListTunnels`) all perform it. Today the response bytes of this particular read happen not to depend on tunnel status, so this is reported as a LOW, no-behavioral-impact divergence: the defect is that one handler of eight does not run the crate's shared pre-read settle step, so `ListTagsForResource` is the only entry point through which a client can touch an expired tunnel's record and have the expiry left unsettled behind it (the record keeps reporting `OPEN` until some other operation is called), and any field this read grows, or any reader that joins its result to a status-derived one, inherits a store state the siblings would have settled.

## Evidence

The handler, with no sweep after the lock — services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/ops/list_tags_for_resource.rs:138-142:
```
        let region = crate::state::request_region(ctx);
        let store = state.lock();
        if store
            .tunnel_id_for_arn(&region, &input.resource_arn)
            .is_none()
```

Every sibling does the opposite, immediately after taking the lock:
- src/ops/describe_tunnel.rs:148-150: `let mut store = state.lock();` / `// Settle the transition the clock already made due before reporting.` / `store.materialize_expired_tunnels(now);`
- src/ops/list_tunnels.rs:168-170: `let mut store = state.lock();` / `// Settle the transition the clock already made due before reporting.` / `store.materialize_expired_tunnels(now);`
- src/ops/tag_resource.rs:153-157 and src/ops/untag_resource.rs:140-144 (the other two ARN-addressed tag handlers): `let mut store = state.lock();` ... `store.materialize_expired_tunnels(now);`
- src/ops/close_tunnel.rs:140-141, src/ops/rotate_tunnel_access_token.rs:213-214, src/ops/open_tunnel.rs:187-191: same two lines.

The crate states the rule the skipped call implements — src/state.rs:557-563, doc of `materialize_expired_tunnels`:
```
    /// Close every tunnel whose `maxLifetimeTimeoutMinutes` has run out.
    ///
    /// This is the one write a reporting operation may make: the transition was
    /// already owed at `now` by the clock — "The maximum amount of time (in
    /// minutes) a tunnel can remain open" — whether or not anybody called.
```
and src/state.rs:8-9 lists it among the state logic "more than one operation needs ... the clock-owed close of a tunnel whose `maxLifetimeTimeoutMinutes` has elapsed".

Test coverage of the shared step today names only the other readers — tests/all/closed_tunnel_shape.rs:22-24 and its `Noticer` enum at closed_tunnel_shape.rs:91-95 (`DescribeTunnel`, `ListTunnels`, `CloseTunnel`): "for the owed close, the operation that first touches the record and so settles it". No test exercises `ListTagsForResource` as a settler, so nothing pins the current omission.

## Steps to Reproduce

1. Open src/ops/describe_tunnel.rs:148-150 and src/ops/list_tunnels.rs:168-170 and note that each reader settles the clock-owed close with `store.materialize_expired_tunnels(now)` as its first act under the lock.
2. Open src/ops/tag_resource.rs:153-157 and src/ops/untag_resource.rs:140-144: the two sibling ARN-addressed tag handlers do the same.
3. Open src/ops/list_tags_for_resource.rs:138-142: the lock is taken as `let store = state.lock();` (not `mut`) and the handler resolves the ARN and returns `store.tag_list(..)` without ever calling `materialize_expired_tunnels`.
4. Mount the service, `OpenTunnel` with `{"timeoutConfig":{"maxLifetimeTimeoutMinutes":1}}`, then back-date the stored record's `expires_at` (as tests/all/closed_tunnel_shape.rs does) so the close is owed.
5. Call `ListTagsForResource` with the tunnel's ARN: it answers 200 with the tags and leaves the record still stored as `OPEN` — the owed transition is settled only by the next `DescribeTunnel`/`ListTunnels`/mutating call, unlike after any sibling operation.
