Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2a76ec95dcbb7025f794fe41b3b77939fee1d4dd

# A node whose owning member is deleted keeps its CreateNode idempotency record, so CreateNode replays a dead node's id

_Filed: 2026-10-27T09:22:39Z_
_Operation: CreateNode | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2a76ec95dcbb7025f794fe41b3b77939fee1d4dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteNode releases the idempotency record of the create that minted the node, under a rule the crate states explicitly: a node whose removal has begun "is not one a replayed token could hand back". The OTHER path that starts a node's removal — DeleteMember, through `begin_member_deletion`, which calls `node.begin_deletion(now)` for every node the member owns — does not release those nodes' tokens. It calls only `forget_dangling_tokens`, whose liveness test `holds_any` keeps a `CreateNode` record alive as soon as the NETWORK is still stored and not deleting — and the network id in that record is one the REQUEST named, not one the create minted, a distinction `forget_node_token`'s own doc draws ("The node is the only id a `CreateNode` mints (the network and member its record also names are ones the request named)").

Net effect: after `CreateNode` (token T) and `DeleteMember` on the owning member while the network still stands (another participating member remains, so the network is not deleted with it), `CreateNode` with token T answers 200 carrying the id of a node that is `DELETING`/`DELETED` instead of minting a fresh node — the exact outcome the DeleteNode path, and the test that pins it, rule out. The handler's replay returns `created.node_id` with no existence or status check, so nothing downstream catches it.

The same shape applies to the deleted MEMBER's own `CreateMember` token (its record also names the request-named network), but the node case is the one the crate's own helper doc states verbatim.

## Evidence

The rule, and the release DeleteNode makes — src/ops/delete_node.rs:260-263:

```
// The idempotency ledger must not outlive the resources it describes: the node is
// the only id the create that minted it named of its own, and a node whose removal
// has begun is not one a replayed token could hand back, so that record goes.
guard.forget_node_token(&input.node_id);
```

and the helper's doc — src/state.rs:2000-2007:

```
/// Forget the record of the create that minted `node_id`.
///
/// Called when the node's deletion STARTS: a node whose removal has begun "can no
/// longer participate on the network" (`Node$Status`), so replaying that token could
/// only hand back a resource no caller can still use — the same stance
/// [`Self::holds_any`] takes. The node is the only id a `CreateNode` mints (the network
/// and member its record also names are ones the request named), so this is the record
/// that create left behind, and a create/delete loop frees it along with the node.
```

The other path that starts the very same removal releases nothing — src/state.rs:2320-2327 (`begin_member_deletion`):

```
member.begin_deletion(now);
// "All associated resources" are deleted with the member, and a node's own
// removal is the same two-stage one.
for node in network.nodes.values_mut() {
    if node.member_id.as_deref() == Some(member_id) {
        node.begin_deletion(now);
    }
}
```

and its caller sweeps only with the generic test — src/ops/delete_member.rs:282-287:

```
// The idempotency ledger must not outlive the resources it describes (the same
// reason a deleted node releases its create token): a create whose every minted id
// is now deleted or deleting could only replay ids nothing a caller can still use.
// Once for the call — the voting path sweeps once for its own removals the same way.
guard.forget_dangling_tokens();
```

which keeps the record because of the request-named network — src/state.rs:2051-2082 (`holds_any`): `let network_held = created.network_id.as_deref().is_some_and(|id| { self.networks.get(id).is_some_and(|network| !network.is_being_deleted()) });` … `network_held || member_held || node_held || proposal_held || accessor_held`, while `CreateNode` stored that network id in the record — src/ops/create_node.rs:428-437: `crate::state::CreatedIds { network_id: Some(input.network_id.clone()), member_id, node_id: Some(node_id.clone()), ... }`.

The replay that then hands the dead id back, unchecked — src/ops/create_node.rs:199-206:

```
// "An idempotent operation completes no more than once" (`ClientRequestToken`):
// a token already used replays the node id its first call minted.
let token = crate::state::idempotency_key(&region, Self::NAME, &input.client_request_token);
if let Some(created) = guard.replay_idempotent(&token) {
    return Ok(CreateNodeOutput {
        node_id: created.node_id.clone(),
    });
}
```

Test coverage: tests/all/network_and_node_lifecycle.rs:1071-1126 (`a_deleted_node_releases_its_idempotency_record`) pins the rule for the DeleteNode path only ("once a node is deleted, its create token mints a new node instead of replaying an id nothing holds"); tests/all/delete_member.rs:379-434 pins the CreateNetwork case. No test reuses a CreateNode token after the owning member was deleted, so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateNetwork and let it settle to AVAILABLE; it has a founding member M1.
2. Add a second owned member M2 to the network (seed or invite flow) and let it settle to AVAILABLE, so deleting M2 will not empty the network.
3. CreateNode on the network with MemberId=M2 and ClientRequestToken="T"; note the returned NodeId.
4. DeleteMember on M2 (200). GetNode on the node reports DELETING, then DELETED once the span is past — its removal has begun.
5. CreateNode again with the same ClientRequestToken="T".
6. Observe 200 carrying the DELETING/DELETED NodeId from step 3. Expected (the rule src/ops/delete_node.rs:260-263 and tests/all/network_and_node_lifecycle.rs:1071 state for the same condition): the token names nothing a caller can still use, so a fresh node is minted.
7. Compare with the DeleteNode path: CreateNode token "U", DeleteNode that node, CreateNode token "U" again -> a freshly minted node id.
