Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# UpdateBridgeSource accepts a NetworkName no network of the bridge's gateway carries, and a FlowArn that resolves to no flow

_Filed: 2026-10-24T13:28:20Z_
_Operation: UpdateBridgeSource | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A bridge network source ingests on one of the networks declared on the gateway the bridge is placed on: `BridgeNetworkSource$NetworkName` is the gateway network's name, the gateway is the one `Bridge.PlacementArn` names, and its networks were declared by CreateGateway (`GatewayNetwork$Name`, "used to reference the network and must be unique among networks in this gateway"). Likewise a bridge flow source names an existing cloud flow (`BridgeFlowSource$FlowArn`).

Expected: PUT /v1/bridges/{BridgeArn}/sources/{SourceName} with `networkSource.networkName` naming a network the placement gateway does not declare is refused (400 BadRequestException, modelled for this operation) and the stored source is unchanged; the same for `flowSource.flowArn` naming a flow the account does not hold (404 NotFoundException is modelled too).

Actual: the handler merges every member it is given straight onto the stored source — `network_name: update.network_name.clone().unwrap_or_else(...)` and `flow_arn: update.flow_arn.clone().unwrap_or_else(...)` — without ever looking at `store.gateways` (the bridge's `placement_arn`) or `store.flows`. A bridge source can therefore be pointed at a nonexistent gateway network or a nonexistent flow and DescribeBridge reports it as a valid source, so a client sees a bridge AWS would never produce. (The same reference hole exists on CreateBridge/AddBridgeSources, which already carry their own open validation issues; this issue is the update path.)

## Evidence

Spec: `smithy_query(query="shape",shape="BridgeNetworkSource")` / `...="UpdateBridgeNetworkSourceRequest")` — `NetworkName` is the gateway network the source uses; `smithy_query(query="shape",shape="GatewayNetwork")` shows those names are declared per gateway, and `ListedBridge$PlacementArn` is "the ARN of the gateway associated with the bridge", so the valid set is resolvable from this store. `smithy_query(query="operation",operation="UpdateBridgeSource")` models BadRequestException (400) and NotFoundException (404).

Handler: `src/ops/update_bridge_source.rs:214-341` — the only refusals are "both wrappers named" (235-241), bridge not found (251), source name not found (258), and wrapper/kind mismatch (269-299). The merges at 274-293 (flow source) and 300-324 (network source) copy `flow_arn`, `network_name`, `multicast_ip`, `port` and `protocol` verbatim; neither `store.gateways` nor `store.flows` is consulted.

Test coverage: `tests/all/update_bridge_source.rs` updates network sources using names that match the fixture gateway, and no case asserts either acceptance or refusal of an unknown gateway network name or an unknown flow ARN.

## Steps to Reproduce

1. POST /v1/gateways with networks [{"name":"net-1","cidrBlock":"10.0.0.0/16"}] and note the gateway ARN
2. POST /v1/bridges with placementArn=<that gateway>, ingressGatewayBridge{maxBitrate,maxOutputs} and sources [{"networkSource":{"name":"net-src","networkName":"net-1","multicastIp":"224.0.0.1","port":5000,"protocol":"rtp"}}]
3. PUT /v1/bridges/{BridgeArn}/sources/net-src with {"networkSource":{"networkName":"no-such-network"}} — observe 202 and the source rewritten, where AWS answers 400 BadRequestException
4. GET /v1/bridges/{BridgeArn} — the source is reported on a gateway network that does not exist
5. On an egress bridge, PUT .../sources/{name} with {"flowSource":{"flowArn":"arn:aws:mediaconnect:us-east-1:000000000000:flow:1-0000000a:nope"}} — accepted 202 for a flow the store does not hold
