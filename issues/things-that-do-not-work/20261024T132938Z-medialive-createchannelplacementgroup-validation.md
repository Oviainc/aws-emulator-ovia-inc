Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# Nodes are stored without being resolved against the cluster's nodes, so a group can be ASSIGNED to a node that does not exist

_Filed: 2026-10-24T13:29:38Z_
_Operation: CreateChannelPlacementGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `Nodes` is "An array of one ID for the Node that you want to associate with the ChannelPlacementGroup", and "The Node and the ChannelPlacementGroup must be in the same Cluster". A create whose Nodes entry names no node of that cluster has nothing to associate, so the real service refuses it (client error, nothing stored) rather than minting a group that claims an association with a nonexistent node.

Actual: `ComAmazonawsMedialiveMedialiveState::insert_channel_placement_group` checks only that the CLUSTER exists, that the store is not full, that the node list is within the documented single-node bound, and that the name is free. The node ids themselves are stored verbatim. The group is then created, moved into the transient `ASSIGNING` state and settled into `ASSIGNED`, and DescribeChannelPlacementGroup / the create response report `nodes: ["9999999"]` for a node the account does not have. The inverse read is silently inconsistent too: a node's `ChannelPlacementGroups` is derived by walking groups that name it, so nothing in the API ever reports the dangling reference as a problem.

## Evidence

Spec: ChannelPlacementGroup `Nodes` member doc ("An array of one ID for the Node that you want to associate...", "The Node and the ChannelPlacementGroup must be in the same Cluster"), and CreateChannelPlacementGroup models BadRequestException (400) and UnprocessableEntityException (422) for a request it cannot process. State: src/state/channel_placement_groups.rs:335-399 — the only refusals are NoSuchCluster, StoreFull, TooManyNodes, NameTaken; `self.nodes` is never consulted. Handler: src/ops/create_channel_placement_group.rs:210-232 passes `input.nodes.unwrap_or_default()` straight through. The sibling update path documents the omission in a comment (src/ops/update_channel_placement_group.rs:211-218: "Resolving a node id against the node slice ... is one change to the create and the update together"). Tests: tests/all/channel_placement_groups.rs asserts the happy path and the node-count bound only; no test asserts acceptance of an unknown node id.

## Steps to Reproduce

1. POST /prod/clusters to create a cluster; note its id (no nodes created in it).
2. POST /prod/clusters/<clusterId>/channelPlacementGroups with {name: "g1", nodes: ["9999999"]}.
3. Expected: a client error (400/422) naming the nodes member, and no group stored.
4. Actual: 201 with nodes: ["9999999"] and state ASSIGNING; GET /prod/clusters/<clusterId>/channelPlacementGroups/<id> later reports state ASSIGNED with the same nonexistent node.
