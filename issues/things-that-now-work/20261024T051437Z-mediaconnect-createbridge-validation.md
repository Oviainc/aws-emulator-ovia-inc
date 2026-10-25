Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# CreateBridge accepts sources and outputs that contradict the bridge's direction (flow source on an ingress bridge, network output on an ingress bridge)

_Filed: 2026-10-24T05:14:37Z_
_Operation: CreateBridge | Class: validation_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A MediaConnect bridge has exactly one direction, and the direction decides what kind of source and output it may carry:

* `AddEgressGatewayBridgeRequest` — "an egress bridge is a cloud-to-ground bridge. The content comes from an existing MediaConnect flow and is delivered to your premises", so an egress bridge's source is a FLOW source (`BridgeFlowSource`) and its outputs are NETWORK outputs delivered to the premises network.
* `AddIngressGatewayBridgeRequest` — "an ingress bridge is a ground-to-cloud bridge. The content originates at your premises and is delivered to the cloud", so an ingress bridge's source is a NETWORK source (`BridgeNetworkSource`) and its outputs are the FLOW outputs MediaConnect materialises when cloud flows attach — which is exactly what `IngressGatewayBridge$MaxOutputs` ("the maximum number of outputs on the ingress bridge", REQUIRED on the create) bounds. `AddBridgeOutputRequest` can only express a `NetworkOutput`, so a client-supplied output on an ingress bridge has no meaning.

Expected: CreateBridge is refused with 400 BadRequestException and nothing stored when (a) an `ingressGatewayBridge` request carries a `sources[].flowSource`, (b) an `egressGatewayBridge` request carries a `sources[].networkSource`, (c) an `ingressGatewayBridge` request carries `outputs` (network outputs), or (d) the request carries more `outputs` than `ingressGatewayBridge.maxOutputs` allows.

Actual: the handler validates only (i) that exactly one of the two bridge-type wrappers is present, (ii) that each source/output entry names exactly one union member, (iii) name uniqueness and (iv) a generic 50-entry collection bound. The kind of each member is never checked against the bridge type, and `maxOutputs` is stored but never used, so the emulator mints bridges AWS cannot create (e.g. an ingress bridge with `maxOutputs: 0` and 50 network outputs) and then reports them on every DescribeBridge/ListBridges.

Secondary symptom of the same class: `MAX_BRIDGE_SOURCES`/`MAX_BRIDGE_OUTPUTS` are the crate's generic 50-entry request bound (`src/state.rs:357-369` admits "the bound is the one this crate already applies to a request-sized collection"), while a real bridge holds a single source or a failover pair.

## Evidence

Spec: `smithy_query(query="shape",shape="AddEgressGatewayBridgeRequest")` — "an egress bridge is a cloud-to-ground bridge. The content comes from an existing MediaConnect flow and is delivered to your premises"; `smithy_query(query="shape",shape="AddIngressGatewayBridgeRequest")` — "an ingress bridge is a ground-to-cloud bridge. The content originates at your premises and is delivered to the cloud", with required member `MaxOutputs` = "the maximum number of expected outputs on the ingress bridge"; `BridgeSource` union members `FlowSource` (BridgeFlowSource) / `NetworkSource` (BridgeNetworkSource) and `AddBridgeOutputRequest` carrying only `NetworkOutput`.

Handler: `src/ops/create_bridge.rs:216-224` checks only that exactly one of `EgressGatewayBridge`/`IngressGatewayBridge` is set; lines 236-297 materialise each source/output through `crate::state::materialise_bridge_source` / `materialise_bridge_output` and check only "exactly one union member", name validity and name uniqueness; lines 344-360 copy `max_bitrate`/`max_outputs` into the stored record and nothing ever compares `outputs.len()` to `max_outputs`.

Test coverage: the existing suite pins the LENIENT behaviour rather than the rule — `tests/all/bridge_lifecycle.rs:172-215` expects 201 for "ingress bridge, one network source, one network output", and `tests/all/bridge_lifecycle.rs:327-345` expects 201 for an ingress bridge with `"maxOutputs": 0` carrying the maximum number of outputs one create may hold. No test asserts a type-mismatch refusal (`tests/all/bridge_lifecycle.rs:554-690` is the refusal table and covers only both-wrappers, no-source, too-many-sources, empty union entries and name rules).

## Steps to Reproduce

1. POST /v1/bridges with {"name":"b-ingress","placementArn":"<gateway arn>","ingressGatewayBridge":{"maxBitrate":20000000,"maxOutputs":2},"sources":[{"flowSource":{"flowArn":"<flow arn>","name":"flow-src"}}]} — observe 201 with a ground-to-cloud bridge fed by a cloud flow, where AWS answers 400 BadRequestException
2. POST /v1/bridges with {"name":"b-egress","placementArn":"<gateway arn>","egressGatewayBridge":{"maxBitrate":80000000},"sources":[{"networkSource":{"multicastIp":"224.0.0.1","name":"net-src","networkName":"net-1","port":5000,"protocol":"rtp"}}]} — observe 201, where AWS answers 400
3. POST /v1/bridges with {"name":"b-ingress-out","placementArn":"<gateway arn>","ingressGatewayBridge":{"maxBitrate":1000,"maxOutputs":0},"sources":[{"networkSource":{...}}],"outputs":[{"networkOutput":{"ipAddress":"10.0.0.5","name":"net-out","networkName":"net-1","port":5001,"protocol":"rtp","ttl":64}}]} — observe 201 with one more output than the bridge declares it can have
4. GET /v1/bridges/{BridgeArn} and observe the contradictory bridge reported back
