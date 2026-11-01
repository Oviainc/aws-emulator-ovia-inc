Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1

# A RequestId spent on a node the cluster has since deregistered still replays that DEREGISTERED node instead of creating a new one

_Filed: 2026-10-29T02:42:59Z_
_Operation: CreateNode | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteNode` is a tombstone delete: the node moves to `DEREGISTERING` and the clock settles it into the terminal `DEREGISTERED`, and the record stays in `self.nodes` so `DescribeNode`/`ListNodes` keep reporting it there. `settle_nodes` materialises that transition but — unlike `settle_clusters` immediately above it, and unlike the channel and network arms of `ComAmazonawsMedialiveMedialiveState::settle` — it never forgets the node's idempotency tokens. `node_for_token` resolves a token through `Self::node`, which carries no state filter at all, so it still answers for the tombstone.

So a later `CreateNode` on the same cluster carrying the `RequestId` of a node that has since been deleted takes the handler's replay branch and answers 201 with the DEREGISTERED node — a node `UpdateNodeState` and `UpdateNode` both refuse with 404 "already deleted", that `resources_in_cluster` no longer counts as being in the cluster, and that `CreateNodeRegistrationScript` refuses as `NodeDeleteAccepted` — while creating and storing nothing.

The handler's own comment beside `record_node_token` asserts the invariant that is missing, and the node slice already owns the predicate the token lookup would need (`NodeRecord::is_delete_accepted`), which every other node mutation resolves against. Only the token path does not.

## Evidence

The rule, from the shared token helper the node token map is an instance of — services/service-com-amazonaws-medialive-medialive/src/state.rs:1790-1792:
```
    /// Drop every token that created `resource_id`, so a token never outlives
    /// the resource it minted.
    fn forget(&mut self, resource_id: &str) {
```

The CreateNode handler ASSERTS exactly that for its own token — src/ops/create_node.rs:230-234:
```
        if let Some(token) = input.request_id {
            // Bounded, evicting write: a caller-chosen token can neither
            // accumulate past the cap nor outlive the node it created.
            state.record_node_token(&input.cluster_id, token, &id);
        }
```
but `settle_nodes` forgets nothing — src/state/nodes.rs:433-441:
```
impl ComAmazonawsMedialiveMedialiveState {
    /// Materialise every node transition the clock had already made due as of
    /// `now`. Called from [`ComAmazonawsMedialiveMedialiveState::settle`], which
    /// every operation runs first.
    pub(super) fn settle_nodes(&mut self, now: SystemTime) {
        for node in self.nodes.values_mut() {
            node.settle(now);
        }
    }
```
while the sibling slice does, in the same words — src/state/clusters.rs:463-480:
```
    pub(super) fn settle_clusters(&mut self, now: SystemTime) {
        ...
        // A token never outlives the cluster it minted: once that cluster has
        // reached the terminal `DELETED` state its idempotency window is over,
        // so a retried create mints a new cluster rather than answering with a
        // deleted one — the rule the channel and network tokens follow too.
        for id in just_deleted {
            self.cluster_tokens.forget(&id);
        }
    }
```
(`grep -n '\.forget(' src` lists `cluster_tokens`, `channel_tokens`, `network_tokens`, `signal_map_tokens` and the four template maps; `node_tokens` is absent.)

The token lookup that therefore still answers for a deregistered node — src/state/nodes.rs:522-540:
```
    /// The node a stored `RequestId` created in `cluster_id`, if that node still
    /// exists in it.
    ...
    pub fn node_for_token(&self, cluster_id: &str, request_id: &str) -> Option<&str> {
        let id = self.node_tokens.created(&node_token_key(cluster_id, request_id))?;
        self.node(cluster_id, id).map(|node| node.id.as_str())
    }

    /// The node `node_id` of the cluster `cluster_id`, whatever state it is in.
```
— "whatever state it is in" is the lookup the replay goes through, while the predicate every node MUTATION resolves against exists right beside it, src/state/nodes.rs:349-363:
```
    /// Whether a delete has already been accepted for this node: it is
    /// deregistering, or has reached the terminal `DEREGISTERED`.
    ///
    /// This is what keeps a node whose delete was accepted READABLE but not
    /// writable: ...
    /// [`ComAmazonawsMedialiveMedialiveState::apply_node_state`] resolves its
    /// node the same way, and so does
    /// [`ComAmazonawsMedialiveMedialiveState::apply_node_changes`].
    pub fn is_delete_accepted(&self) -> bool {
        self.state == node_state::DEREGISTERING || self.state == node_state::DEREGISTERED
    }
```

The replay branch that answers with the tombstone — src/ops/create_node.rs:201-214:
```
        // A retried create (the same `@idempotencyToken` `RequestId`, presented
        // to the same cluster's endpoint) must answer with the node that one
        // created, not a second node.
        if let Some(existing) = input
            .request_id
            .as_deref()
            .and_then(|token| state.node_for_token(&input.cluster_id, token))
            .map(str::to_string)
        {
            let node = state
                .node_description(&input.cluster_id, &existing)
                .ok_or_else(unavailable)?;
            return Ok(crate::state::node_description_output!(CreateNodeOutput, node));
        }
```
and the siblings that prove the replayed id is unusable — src/ops/update_node_state.rs:249-255 (`NodeStateRefusal::DeleteAccepted` -> 404 `already_deleted_message`) and src/ops/create_node_registration_script.rs:254-256 (`Refusal::NodeDeleteAccepted` -> `already_deleted_message`).

Test coverage: the node idempotency table covers only live-node rows and never re-presents a token after a delete — tests/all/node_lifecycle.rs:622-651 (`a_retried_create_returns_the_node_the_first_one_created`, rows "the same token, the same cluster" / "two tokens, the same cluster" / "no token at all" / "only the second names a token" / "the same token, two clusters"), while the same rule past the delete IS pinned for a resource that implements it — tests/all/signal_map_deletion.rs:481-489 (`a_deleted_signal_map_is_not_resurrected_by_its_idempotency_token`).

## Steps to Reproduce

1. POST /prod/clusters with a name and valid networkSettings; poll GET /prod/clusters/{C} until state is ACTIVE.
2. POST /prod/clusters/{C}/nodes with {"name":"node-a","role":"ACTIVE","requestId":"token-a"} -> 201; note the node id N.
3. DELETE /prod/clusters/{C}/nodes/{N} -> the node reports state DEREGISTERING; wait for the transient span and confirm GET /prod/clusters/{C}/nodes/{N} reports state DEREGISTERED (the record is kept, src/state/nodes.rs:365-372).
4. POST /prod/clusters/{C}/nodes again with {"name":"node-b","role":"BACKUP","requestId":"token-a"} -> the emulator answers 201 carrying node N with "state":"DEREGISTERED" and creates nothing; GET /prod/clusters/{C}/nodes shows no new node. Expected, per src/state.rs:1790-1792 and the invariant src/ops/create_node.rs:231-232 asserts, a freshly minted node — which is what POST /prod/clusters gives for a token whose cluster was deleted (src/state/clusters.rs:473-479).
5. PUT /prod/clusters/{C}/nodes/{N}/state with {"state":"ACTIVE"} -> 404 "already deleted" (src/ops/update_node_state.rs:249-255), so the 201 in step 4 named a node the caller can neither use nor change.
