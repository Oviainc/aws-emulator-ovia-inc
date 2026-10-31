Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 355761efac79959eac560a8efce6608069de81f3

# CreateNode stores a node name another node of the same cluster already holds, the collision UpdateNode refuses with 409

_Filed: 2026-10-28T21:22:38Z_
_Operation: CreateNode | Class: validation_

> _Note: this issue was identified against the code base at commit `355761efac79959eac560a8efce6608069de81f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A MediaLive Anywhere node's `Name` is documented as unique within its cluster, and this crate already owns that rule: `ComAmazonawsMedialiveMedialiveState::node_named(cluster_id, name)` resolves the live node of a cluster holding a name, `NodeUpdateRefusal::NameTaken` is the refusal, and `apply_node_changes` (the `UpdateNode` write path) applies it before anything is written, which `ops/update_node.rs` turns into a 409 `ConflictException`.

The CREATE path skips that check entirely. `insert_node` resolves the cluster, bounds `NodeInterfaceMappings`, bounds the store and then inserts the record with `name` verbatim; its refusal enum `NodeRefusal` has only three variants (`ClusterNotFound`, `TooManyInterfaceMappings`, `StoreFull`) and `ops/create_node.rs` has no name check of its own. So `POST /prod/clusters/{ClusterId}/nodes` with a name a live node of that cluster already holds answers 201 and stores a second node under the same name, while the very next `PUT .../nodes/{NodeId}` asking for that same name answers 409.

Two stored nodes sharing one name also corrupt the update check that does exist: `node_named` is a `values().find(...)` over the store, so after a duplicate create it answers with whichever of the two it reaches first. `UpdateNode` on the OTHER one re-sending its own name then sees `holder.id != node_id` and is refused — the exact case `tests/all/node_settings_update.rs` asserts must be accepted ("the node's own name", expected not refused).

Every sibling named resource in this crate enforces its documented uniqueness on the create as well as on the update: `insert_cluster` (`ClusterRefusal::NameAlreadyInUse`), `CreateNetwork` (`network_named` -> `ConflictException`), `CreateChannelPlacementGroup` (`ChannelPlacementGroupRefusal::NameTaken`) and `CreateSignalMap` (`signal_map_named` -> `ConflictException`). `CreateNode` is the one create that does not.

## Evidence

The model states the rule for a node's name — `smithy_query(query="doc", shape="UpdateNodeRequest", member="Name")`:
```
"Include this parameter only if you want to change the current name of the Node. Specify a name that is unique in the Cluster. You can't change the name. Names are case-sensitive."
```

The crate enforces it on the UPDATE — `services/service-com-amazonaws-medialive-medialive/src/state/nodes.rs:888-897`:
```
        // A name another live node of this cluster holds is refused, while
        // re-sending the node's own name is not a collision with itself.
        if let Some(name) = changes.name.as_deref() {
            if self
                .node_named(cluster_id, name)
                .is_some_and(|holder| holder.id != node_id)
            {
                return Err(NodeUpdateRefusal::NameTaken);
            }
        }
```
with the helper at `src/state/nodes.rs:799-805`:
```
        self.nodes.values().find(|node| {
            node.cluster_id == cluster_id
                && !node.is_delete_accepted()
                && node.name.as_deref() == Some(name)
        })
```
and the handler mapping at `src/ops/update_node.rs:252`: `Refusal::NameTaken => UpdateNodeErrorModeled::ConflictException(ConflictException {`.

The CREATE skips it — `src/state/nodes.rs:467-511` (the whole body of `insert_node`): the only guards are `live_cluster`, `interface_mappings.len() > MAX_NODE_INTERFACE_MAPPINGS` and `self.nodes.len() >= MAX_NODES`, then
```
                name,
                role,
                node_interface_mappings: interface_mappings
```
is inserted verbatim. The handler adds nothing: `src/ops/create_node.rs:220-229` calls `state.insert_node(&region, &input.cluster_id, input.name, input.role, ...)` and `src/ops/create_node.rs:256-280` (`fn refusal`) matches only `ClusterNotFound`, `TooManyInterfaceMappings` and `StoreFull`.

The crate's own comment already asserts that a node's `Name` CAN clash — `src/ops/create_node_registration_script.rs:243-245`:
```
/// The `ConflictException` (409) this operation models is left unreached: a
/// script is not a named resource, so nothing it is issued for can clash with one
/// the account already holds the way a cluster's or a node's `Name` does.
```

Siblings that do check on create — `src/state/clusters.rs:571-575`:
```
        if let Some(name) = new.name.as_deref() {
            if self.cluster_named(name).is_some() {
                return Err(ClusterRefusal::NameAlreadyInUse);
            }
        }
```
and `src/ops/create_network.rs:232-248` (`if let Some(existing) = state.network_named(name)` -> `ConflictException`).

Test coverage: the update half is pinned by `tests/all/node_settings_update.rs:372-390` (`update_node_keeps_a_name_unique_in_its_cluster`, rows `("the name a live sibling holds", "sibling", true)` and `("the node's own name", "renamer", false)`). No test exercises two creates with the same name in one cluster — every `create_node` call in `tests/all/node_lifecycle.rs` uses a distinct name (e.g. line 216 `format!("node-{cluster_index}-{count}")`, lines 659/663 `"first"`/`"second"`, line 920 `format!("node-{index}")`), so nothing pins the current acceptance.

## Steps to Reproduce

1. POST /prod/clusters with {"name":"c1","clusterType":"ON_PREMISES"}; note the cluster id
2. POST /prod/clusters/{clusterId}/nodes with {"name":"worker"} — 201, note node id A
3. POST /prod/clusters/{clusterId}/nodes with {"name":"worker"} again (no requestId, or a fresh one) — the emulator answers 201 and stores a SECOND node named "worker"; expected the name-collision refusal every sibling create in this crate makes for its documented uniqueness rule (CreateNode models UnprocessableEntityException, which is what CreateChannelPlacementGroup answers a NameTaken with)
4. GET /prod/clusters/{clusterId}/nodes — two nodes with the same name in one cluster, which UpdateNodeRequest$Name says is impossible
5. PUT /prod/clusters/{clusterId}/nodes/{B}/ with {"name":"worker"} (node B re-sending its OWN name) — refused 409 because `node_named` resolves to node A, contradicting tests/all/node_settings_update.rs row ("the node's own name", not refused)
