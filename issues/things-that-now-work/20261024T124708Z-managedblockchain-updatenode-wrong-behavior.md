Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# UpdateNode applies the configuration change without ever reporting the node as UPDATING

_Filed: 2026-10-24T12:47:08Z_
_Operation: UpdateNode | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `UpdateNode` ("Updates a node configuration with new parameters") starts an asynchronous update, and the node reports it: `Node$Status` documents "UPDATING - The node is in the process of being updated", so a caller that calls UpdateNode and then GetNode observes Status UPDATING before the node settles back to AVAILABLE, and `ListNodes?status=UPDATING` reports it during that window.

Actual: the handler merges the requested `LogPublishingConfiguration` into the stored record and returns, leaving `node.status` exactly as it was (AVAILABLE). `NodeStatus::UPDATING` is written by no handler in the crate, so it is unreachable: GetNode never reports an updating node, and the ListNodes `status=UPDATING` filter can never match. The record already carries the machinery this needs (`StoredNode.status` plus an `available_at` due-instant settled by `StoredNetwork::settle`), which is how CREATING → AVAILABLE is already modeled, so the update window is simply not represented.

## Evidence

Spec: Smithy model `Node$Status` enumerates UPDATING with the doc "UPDATING - The node is in the process of being updated" (and `MemberStatus` carries the matching "UPDATING - The member is in the process of being updated"); `ListNodesInput$Status` accepts UPDATING as a filter value. UpdateNode's own doc is "Updates a node configuration with new parameters."
Handler: services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/update_node.rs lines ~197-209 — the only write is `node.log_publishing_configuration = Some(merged)`; `node.status` and `node.available_at` are untouched, and grep for UPDATING across src/ finds no handler that sets it.
Tests: tests/all/update_node_log_publishing.rs asserts only that GetNode reads the merged configuration back; no test covers the node's status after an update (grep for UPDATING under tests/ finds only seeded member statuses in list_members.rs / delete_member.rs and an error-payload table).

## Steps to Reproduce

1. Create a Hyperledger Fabric network, wait for AVAILABLE, and CreateNode(... InstanceType='bc.t3.small'); wait until GetNode reports Status AVAILABLE.
2. Call UpdateNode(NetworkId=<net>, MemberId=<member>, NodeId=<node>, LogPublishingConfiguration={Fabric:{PeerLogs:{Cloudwatch:{Enabled:true}}}}) — observe 200.
3. Immediately call GetNode: observe Node.Status == 'AVAILABLE'; expected 'UPDATING' until the update settles, then 'AVAILABLE'.
4. Call ListNodes(NetworkId=<net>, MemberId=<member>, Status='UPDATING') right after the update: observe an empty Nodes list; expected the node being updated to be reported.
