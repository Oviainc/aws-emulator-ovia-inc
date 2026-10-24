Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# Updated Nodes are accepted without resolving them against the cluster's nodes

_Filed: 2026-10-24T13:29:38Z_
_Operation: UpdateChannelPlacementGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: an update that rewrites `Nodes` must resolve each id against the nodes of the group's own cluster ("The Node and the ChannelPlacementGroup must be in the same Cluster"); an id naming no node of that cluster is refused with the operation's own client error (it models BadRequestException 400 and UnprocessableEntityException 422) and the stored group is left exactly as it was.

Actual: the handler builds `ChannelPlacementGroupChanges { name, nodes }` and the state slice only length-checks the list before writing it; the ids are stored verbatim. So an update can move a group onto a node that does not exist, which then drives the real ASSIGNING -> ASSIGNED transition and is reported by every later read of the group. The handler's own comment acknowledges the reference is not resolved.

## Evidence

Spec: UpdateChannelPlacementGroup input `Nodes` with the same member documentation as the create, plus modelled BadRequestException (400) / UnprocessableEntityException (422). Handler: src/ops/update_channel_placement_group.rs:209-229 (comment at 211-218 explicitly defers node resolution). State: src/state/channel_placement_groups.rs:463-500 — refusals are NoSuchGroup, AlreadyDeleted, NameTaken, TooManyNodes only; the node store is never consulted before `group.nodes = nodes`. Tests: tests/all/channel_placement_group_update.rs covers rename / node-count / not-found paths, not an unknown node id.

## Steps to Reproduce

1. Create a cluster and a channel placement group in it with no nodes (state UNASSIGNED).
2. PATCH /prod/clusters/<clusterId>/channelPlacementGroups/<groupId> with {nodes: ["9999999"]}.
3. Expected: a 400/422 naming the nodes member; the group stays UNASSIGNED with no nodes.
4. Actual: 200 with nodes: ["9999999"], state ASSIGNING, settling to ASSIGNED — an association with a node the cluster does not have.
