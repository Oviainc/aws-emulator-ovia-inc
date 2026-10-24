Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# CreateNode accepts an Ethereum node with no AvailabilityZone and any InstanceType string

_Filed: 2026-10-24T01:04:38Z_
_Operation: CreateNode | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateNode performs no validation of the two node-sizing/placement inputs the service constrains.

(1) AvailabilityZone. The model documents `NodeConfiguration$AvailabilityZone` as "The Availability Zone in which the node exists. Required for Ethereum nodes." Expected: a CreateNode on an ETHEREUM network (e.g. the standing public `n-ethereum-mainnet`) that omits `AvailabilityZone` is refused with InvalidRequestException (400) and no node is created. Actual: the handler stores `configuration.availability_zone` exactly as given, including `None`, and answers 200 with a freshly minted NodeId; a later GetNode reports `AvailabilityZone: null` for a node AWS could never have placed.

(2) InstanceType. `NodeConfiguration$InstanceType` is "The Amazon Managed Blockchain instance type for the node" — AMB only accepts its own `bc.*` instance types (bc.t3.small/medium/large/xlarge, bc.m5.*, bc.c5.*, with Hyperledger Fabric Starter Edition further restricted to the small/medium sizes). Expected: an instance type AMB does not offer (e.g. `m5.large`, `bc.nonexistent`) is refused with InvalidRequestException (400). Actual: the string is stored verbatim and echoed back by GetNode/ListNodes, so a node exists with an instance type the real service would never provision; the network's stored `fabric_edition` is never consulted either.

Both are over-permissive accepts: callers that branch on InvalidRequestException for a bad node configuration get a 200 plus a node id instead.

## Evidence

Spec: Smithy model `com.amazonaws.managedblockchain#NodeConfiguration` — `AvailabilityZone` doc: "The Availability Zone in which the node exists. Required for Ethereum nodes."; `InstanceType` is @required but typed as the unconstrained `InstanceTypeString`, so the catalogue (bc.* families, Starter-Edition restriction) is service behavior the handler must supply. CreateNode models InvalidRequestException (400).
Handler: services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/create_node.rs — the only configuration checks are the Fabric MemberId requirement and the `StateDB`-on-non-Fabric refusal; `availability_zone: configuration.availability_zone` and `instance_type: configuration.instance_type` are stored unvalidated (with the comment "Stored exactly as the request named it").
Tests: services/.../tests/all/public_networks_and_membership.rs:375-394 creates a node on `n-ethereum-mainnet` with only `{"InstanceType": "bc.t3.large"}` and asserts 200 — i.e. the current, over-permissive behavior is locked in and no test asserts the AvailabilityZone requirement; tests/all/network_and_node_lifecycle.rs:742-800 only exercises valid bc.* instance types and notes `InstanceTypeString` carries no modelled constraint.

## Steps to Reproduce

1. Point a client at the emulator: client = boto3.client('managedblockchain', region_name='us-east-1', endpoint_url=<emulator>)
2. Call create_node(ClientRequestToken='t1', NetworkId='n-ethereum-mainnet', NodeConfiguration={'InstanceType': 'bc.t3.large'}) with NO AvailabilityZone. Observe HTTP 200 with a NodeId; expected 400 InvalidRequestException because AvailabilityZone is required for Ethereum nodes.
3. Call get_node on the returned NodeId and observe Node.AvailabilityZone is absent.
4. Create a Hyperledger Fabric network, wait for AVAILABLE, then call create_node(..., NodeConfiguration={'InstanceType': 'm5.large', 'AvailabilityZone': '<region>a'}). Observe HTTP 200 and GetNode echoing InstanceType 'm5.large'; expected 400 InvalidRequestException for an instance type Managed Blockchain does not offer.
