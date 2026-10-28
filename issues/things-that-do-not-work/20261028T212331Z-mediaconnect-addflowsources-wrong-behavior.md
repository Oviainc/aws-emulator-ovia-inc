Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 355761efac79959eac560a8efce6608069de81f3

# A flow taking a bridge as its source never creates the bridge's flow output, so a ground-to-cloud bridge reports no outputs at all

_Filed: 2026-10-28T21:23:31Z_
_Operation: AddFlowSources | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `355761efac79959eac560a8efce6608069de81f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states — twice in `state.rs`, and in the sentence AddBridgeOutputs sends the CLIENT — that a ground-to-cloud (ingress) bridge's outputs are the `BridgeFlowOutput`s the service itself materialises when a cloud flow takes the bridge as its source (`SetGatewayBridgeSourceRequest`), which is why such a bridge accepts no output in a request.

No path in the crate ever materialises one. AddFlowSources stores the source through `StoredSource::from_set_request`, which copies `GatewayBridgeSource` (bridge ARN included) onto the flow source and touches nothing else; the handler never reaches `store.bridges` at all (the same holds for the other two paths that store this shape, CreateFlow's `Source`/`Sources` and UpdateFlowSource). `materialise_bridge_output` hard-codes `flow_output: None`, and no line in the crate ever assigns `flow_output: Some(..)`.

So an ingress bridge's `Outputs` stays empty for its whole life: DescribeBridge and ListBridges report a bridge with no output after a flow has taken it as its source, and the refusal AddBridgeOutputs answers points the caller at a flow output the service never creates. The flow-output branches the crate carries for bridge outputs (`bridge_output_name`, UpdateBridgeOutput's "NetworkOutput cannot be updated on a flow output of a bridge") are unreachable for the same reason. Everything a `BridgeFlowOutput` needs is already on the stored source: `FlowArn`, `FlowSourceArn` and `Name` are its three required members.

## Evidence

The crate's own claim #1 — services/service-com-amazonaws-mediaconnect-mediaconnect/src/state.rs:596-602:
```
/// * an output a request configures is a `NetworkOutput` ... A ground-to-cloud bridge delivers the
///   other way ("the content originates at your premises and is delivered to the cloud"),
///   so its outputs are the `BridgeFlowOutput`s the service itself materialises when a
///   cloud flow takes the bridge as its source (`SetGatewayBridgeSourceRequest`), and
///   none a request carries; and
```
The same claim sent to the CLIENT as the refusal body — src/state.rs:622-629:
```
    if ingress_max_outputs.is_some() {
        return Some(
            "An output a request configures is a network output ... This bridge is a \
             ground-to-cloud bridge: its outputs are the flow outputs created for it when a \
             flow takes the bridge as its source, so it accepts no output in a request."
```
(reached from src/ops/add_bridge_outputs.rs:220-232 and src/ops/create_bridge.rs:311.)

The crate's own claim #2, beside the code that always stores no flow output — src/state.rs:2916-2923:
```
/// that names none configures no output at all. A `BridgeOutput` can also be a
/// `BridgeFlowOutput`, which no request shape in this model configures: a bridge's flow
/// output is the one a cloud flow creates when it takes the bridge as its source
/// (`SetGatewayBridgeSourceRequest`), never one a create or add request carries.
pub fn materialise_bridge_output(request: &AddBridgeOutputRequest) -> Option<BridgeOutput> {
    let network_output = request.network_output.as_ref()?;
    Some(BridgeOutput {
        flow_output: None,
```
What the write path actually does — src/state.rs:1545-1552 (the bridge ARN is copied onto the flow source and nothing more):
```
            // `SetGatewayBridgeSourceRequest` mirrors `GatewayBridgeSource` member for
            // member, with `BridgeArn` required on both.
            gateway_bridge_source: request.gateway_bridge_source.as_ref().map(|bridge| {
                GatewayBridgeSource {
                    bridge_arn: bridge.bridge_arn.clone(),
```
called from src/ops/add_flow_sources.rs:300-311, whose only writes are to the flow:
```
        for (request, name) in input.sources.iter().zip(names) {
            let suffix = store.next_suffix();
            added.push(StoredSource::from_set_request(request, name, &region, suffix));
        }
        let Some(flow) = store.flows.get_mut(&input.flow_arn) else {
...
        flow.sources.extend(added);
```
`grep -rn "flow_output" src/` finds no assignment of `Some(..)` anywhere: only `flow_output: None` (state.rs:2923) and the carry-over of the always-None stored value in src/ops/update_bridge_output.rs:300 (`flow_output: stored.flow_output.clone()`).

Model, `BridgeFlowOutput` (smithy_query query="shape"): "The output of the bridge. A flow output is delivered to the Amazon Web Services cloud." with required `FlowArn` ("The ARN of the cloud flow"), `FlowSourceArn` ("The ARN of the flow source") and `Name`; `SetGatewayBridgeSourceRequest$BridgeArn` is "The ARN of the bridge feeding this flow."

Test coverage: no test reads a bridge's outputs after a flow takes it as its source — tests/all/update_flow_source.rs:275-295 and tests/all/flow_name_references.rs:316 only send `gatewayBridgeSource` and assert the FLOW side; tests/all/update_bridge_output.rs:337-343 carries a `flowOutput` reader that no fixture can ever populate.

## Steps to Reproduce

1. POST /v1/gateways with one network — 201; keep GatewayArn (wait out the transition span so it is ACTIVE).
2. POST /v1/bridges with {"placementArn":GatewayArn,"ingressGatewayBridge":{"maxBitrate":20000000,"maxOutputs":2},"sources":[{"networkSource":{...}}]} — 201; keep BridgeArn (a ground-to-cloud bridge).
3. POST /v1/flows with a minimal source — 201; keep FlowArn.
4. POST /v1/flows/{FlowArn}/source with {"sources":[{"name":"from-bridge","gatewayBridgeSource":{"bridgeArn":BridgeArn}}]} (AddFlowSources) — 201, and the stored source reports gatewayBridgeSource.bridgeArn == BridgeArn.
5. GET /v1/bridges/{BridgeArn} (DescribeBridge) — observe `outputs: []`: no `flowOutput` naming FlowArn and the new source's ARN, although the crate states the service materialises one "when a cloud flow takes the bridge as its source".
6. POST /v1/bridges/{BridgeArn}/outputs with a `networkOutput` — observe 400 with "its outputs are the flow outputs created for it when a flow takes the bridge as its source, so it accepts no output in a request", i.e. the bridge can hold no output of either kind, ever.
