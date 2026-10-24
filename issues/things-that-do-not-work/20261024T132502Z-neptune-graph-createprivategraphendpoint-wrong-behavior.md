Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# CreatePrivateGraphEndpoint answers 501 when vpcId/subnetIds are omitted instead of defaulting them

_Filed: 2026-10-24T13:25:02Z_
_Operation: CreatePrivateGraphEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`vpcId` and `subnetIds` are OPTIONAL input members of CreatePrivateGraphEndpoint: AWS defaults `vpcId` to the caller's default VPC and `subnetIds` to that VPC's subnets, and answers 201 with the created endpoint (vpcId, subnetIds, status CREATING, vpcEndpointId).

The handler instead short-circuits any request that does not carry BOTH members and returns the scaffold's 501 NotImplemented (`crate::scaffolded::Scaffolded::not_implemented("CreatePrivateGraphEndpoint without vpcId and subnetIds ...")`), so the plainest form of the call — `create-private-graph-endpoint --graph-identifier g-xxxxxxxxxx` — fails with an unmodelled 501 and persists nothing. Expected: 201 and a stored endpoint record addressable by GetPrivateGraphEndpoint / DeletePrivateGraphEndpoint, with a service-chosen vpcId and a non-empty subnetIds list (the emulator can mint stable synthetic `vpc-`/`subnet-` ids the way it already mints `vpce-` ids). Note this is NOT the scaffold-stub gap: the operation is implemented for requests that name both members, only this documented default-from-VPC facet is missing.

## Evidence

Spec: smithy_query(operation="CreatePrivateGraphEndpoint") shows `vpcId` (pattern ^vpc-[a-z0-9]+$) and `subnetIds` (len 1..6) as OPTIONAL inputs while the output declares `vpcId`, `subnetIds` and `status` as required, and the member docs describe the service defaulting them from the account's VPC inventory; the operation models no error for an absent vpcId, and 501/NotImplemented is not a modelled response at all.
Handler: src/ops/create_private_graph_endpoint.rs lines ~172-185 — `let (Some(vpc_id), Some(subnet_ids)) = (input.vpc_id, input.subnet_ids) else { return Err(crate::scaffolded::Scaffolded::not_implemented(...)) };` — the rest of the handler (conflict check, quota check, graph-status gate, record insert) is fully implemented.
Test coverage: tests/all/private_graph_endpoint_lifecycle.rs::a_create_that_asks_the_service_to_default_the_vpc_is_not_implemented (line ~831) asserts the 501, i.e. the missing facet is locked in by a test; no test covers a successful create without vpcId.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName=endpoint-probe, provisionedMemory=16) and poll GetGraph until status is AVAILABLE.
3. CreatePrivateGraphEndpoint(graphIdentifier=<id>) with no vpcId and no subnetIds.
4. Observe HTTP 501 NotImplemented and no endpoint stored (ListPrivateGraphEndpoints/GetPrivateGraphEndpoint find nothing); expected 201 with a service-defaulted vpcId, a non-empty subnetIds list, status CREATING and a vpcEndpointId, readable afterwards through GetPrivateGraphEndpoint.
5. Repeat with vpcId and subnetIds supplied: the call succeeds, showing only the defaulting facet is missing.
