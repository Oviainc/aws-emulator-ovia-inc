Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0

# CreateNode silently drops MemberId on a non-Fabric (public Ethereum) network instead of refusing the request

_Filed: 2026-10-24T16:19:31Z_
_Operation: CreateNode | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `CreateNodeInput$MemberId` is "The unique identifier of the member that owns this node. Applies only to Hyperledger Fabric." An Ethereum network (the AWS-owned public networks n-ethereum-mainnet / n-ethereum-goerli this emulator serves) holds no members at all, so a CreateNode that names a MemberId there names a member that cannot exist and must be refused — CreateNode models both InvalidRequestException (400) and ResourceNotFoundException (404) for exactly this kind of unresolvable input, and this is how the emulator itself already answers the sibling reads: ListNodes, GetNode and DeleteNode all refuse a memberId on a non-Fabric network (ListNodes with the recorded AWS-style 'Member with Id m-… not found in network n-…' InvalidRequestException).

Actual: the handler only consults MemberId when the network's framework is HYPERLEDGER_FABRIC (`let member_id = if framework == FRAMEWORK_HYPERLEDGER_FABRIC { … } else { None };`). On any other framework the member is dropped on the floor with no validation: the node is created and persisted with `member_id: None` (and therefore no inherited KmsKeyArn), CreateNode answers 200 with a NodeId, and a subsequent GetNode reports a Node with no MemberId — so the caller's documented input vanished without error and without being stored. It also leaves the crate inconsistent: the same request shape is a 400 on ListNodes/GetNode/DeleteNode and a silent 200 on CreateNode.

## Evidence

Spec: smithy_query(doc, shape=CreateNodeInput, member=MemberId) — "The unique identifier of the member that owns this node. Applies only to Hyperledger Fabric."; CreateNode models InvalidRequestException(400) and ResourceNotFoundException(404). The only non-Fabric networks in the store are the AWS-owned public Ethereum ones seeded by src/state.rs PUBLIC_NETWORKS, which carry an empty `members` map.
Handler: services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/create_node.rs lines ~247-285 — the member branch (existence check + ResourceNotFoundException + ResourceNotReadyException) runs only for HYPERLEDGER_FABRIC; the `else { None }` arm discards `input.member_id` without any check, and the stored `StoredNode.member_id` is then None (lines ~377-403).
Contrast in the same crate: src/ops/list_nodes.rs lines ~230-232 and src/ops/get_node.rs / delete_node.rs refuse a MemberId that the (non-Fabric) network cannot own.
Tests: tests/all/node_configuration_validation.rs and network_and_node_lifecycle.rs cover the Fabric member rules and the ListNodes/GetNode/DeleteNode memberId refusals; grep finds no test that creates an Ethereum node with a MemberId, so this path is uncovered.

## Steps to Reproduce

1. Point a botocore managedblockchain client at the emulator (region us-east-1).
2. Call create_node(ClientRequestToken=<token>, NetworkId='n-ethereum-mainnet', MemberId='m-'+'0'*30, NodeConfiguration={'InstanceType': 'bc.m5.large', 'AvailabilityZone': 'us-east-1a'}).
3. Observe HTTP 200 with a freshly minted NodeId; expected a refusal (400 InvalidRequestException / 404 ResourceNotFoundException) naming the member that the Ethereum network cannot own, as list_nodes(NetworkId='n-ethereum-mainnet', MemberId='m-'+'0'*30) is already refused.
4. Call get_node(NetworkId='n-ethereum-mainnet', NodeId=<new id>) and observe that the returned Node carries no MemberId at all — the input member was neither stored nor rejected.
