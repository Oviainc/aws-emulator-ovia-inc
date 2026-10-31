Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 9f84f44c6767c5c874b7cc6e16fb6469b8a0f678

# DeleteFlow drops a flow that a bridge still uses as its flow source, leaving BridgeFlowSource.FlowArn naming nothing

_Filed: 2026-10-28T23:43:18Z_
_Operation: DeleteFlow | Class: validation_

> _Note: this issue was identified against the code base at commit `9f84f44c6767c5c874b7cc6e16fb6469b8a0f678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteFlow removes the flow record and then repairs ONLY the router-side references to it (`clear_unresolvable_flow_references`, which walks `router_inputs` and `router_outputs` and nothing else). A bridge's flow source names a flow too — `BridgeFlowSource$FlowArn`, "The ARN of the cloud flow used as a source of this bridge" — and that member is `required: true`, so the reference cannot be nulled out the way the OPTIONAL router members are.

After DeleteFlow, the stored bridge keeps a flow source whose `flowArn` resolves to nothing: DescribeBridge/ListBridges keep reporting it, and re-stating that very ARN through UpdateBridgeSource (or adding the same source through AddBridgeSources) is refused 400, because both resolve `FlowSource FlowArn` against `store.flows`. So DeleteFlow manufactures exactly the state the crate's own bridge write paths refuse to store, and the state module itself says what should happen for a REQUIRED reference: the delete of its target is refused instead (that is how DeleteRouterNetworkInterface handles its own required reference, through `associated_router_resource_rejection`). DeleteFlow does neither: it neither refuses nor repairs.

## Evidence

The delete repairs only the router half — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/delete_flow.rs:197-203:
```
        let flow_arn = flow.flow_arn.clone();
        store.flows.remove(&input.flow_arn);
        // The flow goes, and its sources and outputs go with it, so every record of this
        // store that NAMED one of them stops naming it: the references go with the resources
        // they pointed at, through the one repair every operation that takes a flow, a flow
        // output or a flow source out of the store performs.
        store.clear_unresolvable_flow_references(now);
```
and that repair only touches `router_inputs` / `router_outputs` — src/state.rs:5953-6026:
```
    pub fn clear_unresolvable_flow_references(&mut self, now: SystemTime) {
...
        let unresolved_inputs: Vec<(String, bool, bool)> = self
            .router_inputs
...
        let unresolved_outputs: Vec<(String, bool, bool)> = self
            .router_outputs
```
The crate's own stated rule for a reference the model makes REQUIRED — src/state.rs:5946-5948:
```
    /// ([`Self::settle_due_transitions`]). A reference the model makes REQUIRED cannot be
    /// dropped off a record, which is why taking its target away is refused instead
    /// ([`Self::associated_router_resource_rejection`]).
```
The model makes this one required — `smithy_query(query="shape",shape="BridgeFlowSource")`:
```
"FlowArn": {"target": "String", "jsonName": "flowArn", "clientOptional": true, "required": true, "doc": "<p> The ARN of the cloud flow used as a source of this bridge.</p>"}
```
The bridge write paths refuse exactly this dangling reference — src/state.rs:5775-5786:
```
    pub fn bridge_source_reference_rejection(
        &self,
        placement_arn: &str,
        source: &BridgeSource,
    ) -> Option<String> {
        if let Some(flow_source) = source.flow_source.as_ref() {
            if let Some(rejection) =
                self.unknown_flow_rejection("FlowSource FlowArn", &flow_source.flow_arn)
```
called from src/ops/add_bridge_sources.rs:255-264, whose comment states the invariant DeleteFlow breaks:
```
            // A bridge source names other resources: the cloud flow a flow source takes its
            // content from (`BridgeFlowSource$FlowArn`) ... Both are RESOLVED
            // against the store before the source is stored, through the one function every
            // path that writes a source onto a stored bridge resolves them with — so this
            // operation stores no source UpdateBridgeSource would refuse
            if let Some(detail) = store.bridge_source_reference_rejection(&placement_arn, &source) {
```
and from src/ops/update_bridge_source.rs:295-299:
```
            if let Some(flow_arn) = update.flow_arn.as_deref() {
                if let Some(rejection) = store.unknown_flow_rejection("FlowSource FlowArn", flow_arn)
                {
                    return Err(bad_request(rejection));
                }
```
Test coverage: tests/all/bridge_add_references.rs:553 pins the add-side refusal (`json!({ "flowSource": { "flowArn": unheld } })`), and tests/all/flow_removal_clears_router_references.rs pins the removal repair for the ROUTER holders only — its own header says the rule is about "a stored reference ... which the model makes OPTIONAL" and enumerates only router inputs and router outputs. No test covers deleting a flow a bridge names.

## Steps to Reproduce

1. POST /v1/gateways to create a gateway; keep GatewayArn.
2. POST /v1/flows with one source — 201; keep FlowArn.
3. POST /v1/bridges with {"name":"b1","placementArn":GatewayArn,"egressGatewayBridge":{"maxBitrate":80000000},"sources":[{"flowSource":{"flowArn":FlowArn,"name":"flow-src"}}]} — 201; keep BridgeArn.
4. StopFlow is not needed (the flow is STANDBY). DELETE /v1/flows/{FlowArn} — observe 202 and the flow gone.
5. GET /v1/bridges/{BridgeArn} — observe the bridge still reporting sources[0].flowSource.flowArn == FlowArn, an ARN the store no longer holds.
6. PUT /v1/bridges/{BridgeArn}/sources/flow-src with {"flowSource":{"flowArn":FlowArn}} — observe 400 BadRequestException "FlowSource FlowArn ... does not name a flow this account holds": the stored source can no longer be restated.
7. Control: POST /v1/bridges/{BridgeArn}/sources with the same {"flowSource":{"flowArn":FlowArn,...}} is likewise refused 400 — proving the delete created a source the crate's own write paths reject.
