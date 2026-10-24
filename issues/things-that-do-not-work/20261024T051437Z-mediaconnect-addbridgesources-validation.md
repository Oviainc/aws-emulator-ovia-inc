Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# AddBridgeSources accepts a source whose kind contradicts the bridge's direction

_Filed: 2026-10-24T05:14:37Z_
_Operation: AddBridgeSources | Class: validation_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A bridge's direction decides the kind of source it ingests: an egress bridge is "a cloud-to-ground bridge. The content comes from an existing MediaConnect flow" (so its sources are `BridgeFlowSource`s), and an ingress bridge is "a ground-to-cloud bridge. The content originates at your premises" (so its sources are `BridgeNetworkSource`s).

Expected: 400 BadRequestException with the bridge unchanged when the request adds a `flowSource` to an ingress bridge or a `networkSource` to an egress bridge, and when the added source would give the bridge more sources than a bridge can ingest (a single source, or a failover pair).

Actual: the handler checks only that the batch is non-empty, that the count stays within the crate's generic 50-entry `MAX_BRIDGE_SOURCES`, that each entry names exactly one of `FlowSource`/`NetworkSource`, and that names do not collide. It never compares the source kind with `bridge.egress_gateway_bridge`/`bridge.ingress_gateway_bridge`, so one bridge can end up mixing cloud-flow sources and premises network sources — a bridge AWS will not produce — and DescribeBridge then reports it.

## Evidence

Spec: `smithy_query(query="shape",shape="AddEgressGatewayBridgeRequest")` and `...="AddIngressGatewayBridgeRequest"` state the direction and content origin of each bridge type; `BridgeSource` is a union of `FlowSource` (BridgeFlowSource, "the source of a bridge ... an existing cloud flow") and `NetworkSource` (BridgeNetworkSource, a premises source).

Handler: `src/ops/add_bridge_sources.rs:197-256` — bounds are `input.sources.is_empty()`, `held + input.sources.len() > crate::state::MAX_BRIDGE_SOURCES` (50, which `src/state.rs:357-365` documents as the crate's own generic bound: "AWS publishes no count for a bridge's sources"), the union "exactly one of FlowSource and NetworkSource" check, name validity and name uniqueness. The stored bridge's type is never consulted.

Test coverage: none for the pairing rule. `tests/all/bridge_members_and_deletion.rs` adds network sources to an ingress fixture bridge and `tests/all/update_bridge_source.rs:65-73` does the same; no case adds a mismatched kind, so nothing asserts either the lenient behaviour or the refusal for this path.

## Steps to Reproduce

1. POST /v1/bridges with {"name":"b-ingress","placementArn":"<gateway arn>","ingressGatewayBridge":{"maxBitrate":20000000,"maxOutputs":2},"sources":[{"networkSource":{"multicastIp":"224.0.0.1","name":"net-src","networkName":"net-1","port":5000,"protocol":"rtp"}}]}
2. POST /v1/bridges/{BridgeArn}/sources with {"sources":[{"flowSource":{"flowArn":"<flow arn>","name":"flow-src"}}]} — observe 202 and a ground-to-cloud bridge that now also ingests a cloud flow, where AWS answers 400 BadRequestException
3. GET /v1/bridges/{BridgeArn} and observe the bridge carrying both a networkSource and a flowSource
