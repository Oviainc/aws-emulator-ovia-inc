Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# DeleteNode removes the node record outright instead of moving it to DELETING and then DELETED

_Filed: 2026-10-24T12:47:08Z_
_Operation: DeleteNode | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `DeleteNode` starts an asynchronous deletion. The node stays readable and reports the deletion through its status — `NodeStatus` declares `DELETING` ("The node is in the process of being deleted") and `DELETED` ("The node can no longer participate on the network") exactly for this — so a caller that deletes a node and then polls `GetNode` sees 200 with `Status: DELETING`, and later `Status: DELETED`; `ListNodes` reports the node while it is in those statuses, which is why `ListNodesInput$Status` accepts `DELETING` and `DELETED` as filter values.

Actual: the handler erases the record synchronously (`network.nodes.remove(&input.node_id)`), so the very next `GetNode` is a 404 `ResourceNotFoundException` and `ListNodes` no longer reports the node at all. Consequences: (a) the standard delete-then-poll-until-DELETED workflow (and any SDK waiter built on it) fails with a not-found error instead of observing the terminal status; (b) `NodeStatus::DELETING` and `NodeStatus::DELETED` are unreachable from every handler in the crate, so `ListNodes?status=DELETING` / `status=DELETED` can never match anything.

Note the same crate already models exactly this shape of deletion correctly for accessors (`StoredAccessor::begin_deletion` → `PENDING_DELETION`, settled to `DELETED` on the clock, with the record kept readable), so the divergence is specific to nodes (and members, filed separately).

## Evidence

Spec: Smithy model `com.amazonaws.managedblockchain#NodeStatus` = CREATING | AVAILABLE | UNHEALTHY | CREATE_FAILED | UPDATING | DELETING | DELETED | FAILED | INACCESSIBLE_ENCRYPTION_KEY; `Node$Status` documents "DELETING - The node is in the process of being deleted" and "DELETED - The node can no longer participate on the network" — statuses that are only observable if the record survives the delete call. `ListNodesInput$Status` (NodeStatus) exposes them as listing filters.
Handler: services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/delete_node.rs lines ~212-240: after the `AVAILABLE` precondition it does `network.nodes.remove(&input.node_id)` plus `guard.forget_node_token(..)` and answers 200. `StoredNode` carries `status` and `available_at` (state.rs ~725-737) but no deletion-due instant, and `StoredNetwork::settle` (state.rs ~544-548) only settles CREATING → AVAILABLE for nodes.
Contrast in the same crate: `StoredAccessor::begin_deletion` / `StoredAccessor::settle` (state.rs ~968-987) keep the deleted accessor readable through PENDING_DELETION → DELETED.
Tests: no test asserts a node in DELETING/DELETED after a delete — grep for DELETING under tests/ finds only seeded member statuses (tests/all/list_members.rs, tests/all/delete_member.rs) and the accessor suite's PENDING_DELETION cases; tests/all/network_and_node_lifecycle.rs and tests/all/delete_member.rs assert the opposite (immediate 404 after the delete).

## Steps to Reproduce

1. Create a Hyperledger Fabric network and wait for it (and its founding member) to reach AVAILABLE.
2. Call CreateNode(NetworkId=<net>, MemberId=<member>, NodeConfiguration={InstanceType: 'bc.t3.small'}) and wait until GetNode reports Status AVAILABLE.
3. Call DeleteNode(NetworkId=<net>, MemberId=<member>, NodeId=<node>) — observe 200.
4. Immediately call GetNode(NetworkId=<net>, MemberId=<member>, NodeId=<node>): observe 404 ResourceNotFoundException; expected 200 with Node.Status == 'DELETING' (settling to 'DELETED').
5. Call ListNodes(NetworkId=<net>, MemberId=<member>, Status='DELETING') and then Status='DELETED': both return an empty Nodes list; expected the deleted node to be reported under the status it currently holds.
