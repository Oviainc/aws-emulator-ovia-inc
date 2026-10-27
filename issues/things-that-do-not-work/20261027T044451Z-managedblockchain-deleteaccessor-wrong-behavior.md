Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f9b86d9948f04d96bbe5506e2764e861468ce8ee

# DeleteAccessor never releases its create's idempotency record, so CreateAccessor replays a deleted accessor's id and BillingToken

_Filed: 2026-10-27T04:44:51Z_
_Operation: DeleteAccessor | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f9b86d9948f04d96bbe5506e2764e861468ce8ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other delete in this crate releases the idempotency ledger record of the create that minted the resource, under an explicitly stated rule ("the idempotency ledger must not outlive the resources it describes"). `DeleteAccessor` calls neither `forget_node_token` nor `forget_dangling_tokens`, and the ledger's own liveness test is broken for accessors on top of that:

* `holds_any` is documented as "Whether any resource `created` names is still stored AND has not begun being deleted", and its network/member/node branches each test `!is_being_deleted()`. Its accessor branch tests only `self.accessors.contains_key(id)` — no deletion check — so a `PENDING_DELETION` or `DELETED` accessor keeps its token alive even when some other call does sweep the ledger.
* An accessor record is never removed from the store (`settle` only flips its status; no `accessors.remove` exists anywhere in the crate), so `CreateAccessor`'s guard "A ledger record whose accessor is no longer stored is NOT replayed" can never fire for a deleted accessor.

Net effect: `CreateAccessor` with a token already used, after `DeleteAccessor` ran, answers 200 with the id, `BillingToken` and `NetworkType` of an accessor that is `PENDING_DELETION`/`DELETED` — a billing token the service's own documentation says "can't be used for new WebSocket requests or HTTP requests" — instead of minting a fresh accessor the way the identical network/member/node flow does.

## Evidence

`DeleteAccessor`'s whole write path, with no ledger sweep — src/ops/delete_accessor.rs:168-186:

```
let Some(accessor) = guard.accessor_mut(&region, &input.accessor_id) else { ... };
...
accessor.begin_deletion(now);

Ok(DeleteAccessorOutput {})
```

The two sibling deletes do sweep, and say why — src/ops/delete_node.rs:260-263:

```
// The idempotency ledger must not outlive the resources it describes: the node is
// the only id the create that minted it named of its own, and a node whose removal
// has begun is not one a replayed token could hand back, so that record goes.
guard.forget_node_token(&input.node_id);
```

and src/ops/delete_member.rs:282-287:

```
// The idempotency ledger must not outlive the resources it describes (the same
// reason a deleted node releases its create token): a create whose every minted id
// is now deleted or deleting could only replay ids nothing a caller can still use.
guard.forget_dangling_tokens();
```

`holds_any` contradicts its own doc for accessors — src/state.rs:2018-2024 and 2026-2055:

```
/// Whether any resource `created` names is still stored AND has not begun being
/// deleted.
///
/// A record whose removal has started stays readable so a caller can watch it reach
/// `DELETED`, but it is no longer a resource a replayed create token could hand back:
/// the ledger must not outlive the resources it describes, so such a record does not
/// keep its token alive.
fn holds_any(&self, created: &CreatedIds) -> bool {
    let network_held = created.network_id.as_deref().is_some_and(|id| {
        self.networks.get(id).is_some_and(|network| !network.is_being_deleted())
    });
    ...
    let accessor_held = created
        .accessor_id
        .as_deref()
        .is_some_and(|id| self.accessors.contains_key(id));
```

`CreateAccessor`'s replay guard that can therefore never fire — src/ops/create_accessor.rs:205-218:

```
let replayed = guard
    .replay_idempotent(&token)
    .and_then(|created| created.accessor_id)
    .and_then(|accessor_id| guard.accessor(&region, &accessor_id).cloned());
if let Some(accessor) = replayed {
    // ... A ledger record whose accessor is no longer stored is NOT replayed: it
    // could only hand back a token nothing holds, so such a token creates a new
    // accessor below.
    return Ok(CreateAccessorOutput { ... });
```

An accessor record is never dropped: state.rs:1729-1733 settles only its status, and the crate contains no `accessors.remove` (the only `.remove(` calls are on `idempotency` at state.rs:1968/1990/2013 and on a tag map at untag_resource.rs:217).

Test coverage: tests/all/delete_member.rs:379-434 (`a_create_token_is_released_only_once_nothing_it_minted_is_left`) pins exactly this rule for the network/member case; tests/all/accessor_lifecycle.rs:259-323 only replays a token while the accessor is still `AVAILABLE`, so nothing pins the post-delete accessor replay.

## Steps to Reproduce

1. `CreateAccessor` with `ClientRequestToken` "t1", `AccessorType` "BILLING_TOKEN". Note the returned `AccessorId` and `BillingToken`.
2. `DeleteAccessor` on that `AccessorId`. `GetAccessor` now reports `PENDING_DELETION` (and `DELETED` after `ACCESSOR_DELETION_SPAN`).
3. `CreateAccessor` again with the same `ClientRequestToken` "t1".
4. Observe 200 carrying the DELETED accessor's `AccessorId` and `BillingToken`. Expected (the rule the sibling deletes and tests/all/delete_member.rs:379 pin): the token names nothing a caller can still use, so a fresh accessor is minted.
5. Compare with the network path: `CreateNetwork` token "t2", `DeleteMember` its last member, `CreateNetwork` token "t2" again -> a freshly minted network id.
