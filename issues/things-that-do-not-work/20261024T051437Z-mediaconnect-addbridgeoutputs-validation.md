Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# AddBridgeOutputs adds network outputs to a ground-to-cloud (ingress) bridge and ignores the bridge's declared MaxOutputs

_Filed: 2026-10-24T05:14:37Z_
_Operation: AddBridgeOutputs | Class: validation_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AddBridgeOutputs adds `AddBridgeOutputRequest` entries, and that request shape can only express a `NetworkOutput` — an output delivered to the premises network, which is what a cloud-to-ground (egress) bridge produces. A ground-to-cloud (ingress) bridge delivers into the cloud: its outputs are the `BridgeFlowOutput`s MediaConnect materialises when cloud flows attach, and how many it may have is the bridge's own REQUIRED `IngressGatewayBridge$MaxOutputs` ("the maximum number of outputs on the ingress bridge").

Expected: 400 BadRequestException, with the bridge unchanged, when the addressed bridge is an ingress bridge (a client cannot add network outputs to it) and, for a bridge that does accept outputs, when the resulting output count would exceed the maximum the bridge declares.

Actual: the handler resolves the bridge, bounds the batch only by the crate's generic 50-entry ceiling `MAX_BRIDGE_OUTPUTS`, checks name uniqueness, and appends. It never looks at `bridge.ingress_gateway_bridge` / `bridge.egress_gateway_bridge` and never reads `max_outputs`, so an ingress bridge that declares `maxOutputs: 0` can be grown to 50 network outputs, all of which DescribeBridge then reports.

## Evidence

Spec: `smithy_query(query="shape",shape="IngressGatewayBridge")` — required member `MaxOutputs` = "the maximum number of outputs on the ingress bridge"; `smithy_query(query="shape",shape="AddIngressGatewayBridgeRequest")` — "an ingress bridge is a ground-to-cloud bridge. The content originates at your premises and is delivered to the cloud"; `AddBridgeOutputRequest` declares only `NetworkOutput`, and `BridgeOutput` declares `FlowOutput` beside it for the outputs the service itself creates.

Handler: `src/ops/add_bridge_outputs.rs:197-253` — the only bounds are `input.outputs.is_empty()`, `held + input.outputs.len() > crate::state::MAX_BRIDGE_OUTPUTS` (50, documented in `src/state.rs:367-369` as the crate's generic request bound rather than an AWS number), the "must name a NetworkOutput" union check and name uniqueness. `max_outputs` appears nowhere in the handler; `src/state.rs` keeps it only to echo it back.

Test coverage: `tests/all/bridge_members_and_deletion.rs:110-124` builds its fixture bridge as an INGRESS bridge (`"maxOutputs": 8`) and then adds network outputs to it expecting success, i.e. the suite pins the lenient behaviour; no test asserts a refusal for an ingress bridge or for exceeding the declared maximum.

## Steps to Reproduce

1. POST /v1/bridges with {"name":"b-ingress","placementArn":"<gateway arn>","ingressGatewayBridge":{"maxBitrate":20000000,"maxOutputs":1},"sources":[{"networkSource":{"multicastIp":"224.0.0.1","name":"net-src","networkName":"net-1","port":5000,"protocol":"rtp"}}]}
2. POST /v1/bridges/{BridgeArn}/outputs with {"outputs":[{"networkOutput":{"ipAddress":"10.0.0.5","name":"o1","networkName":"net-1","port":5001,"protocol":"rtp","ttl":64}},{"networkOutput":{"ipAddress":"10.0.0.6","name":"o2","networkName":"net-1","port":5002,"protocol":"rtp","ttl":64}}]} — observe 202 with both network outputs added to a ground-to-cloud bridge whose declared maximum is 1, where AWS refuses with 400 BadRequestException
3. GET /v1/bridges/{BridgeArn} and observe two network outputs on an ingress bridge
