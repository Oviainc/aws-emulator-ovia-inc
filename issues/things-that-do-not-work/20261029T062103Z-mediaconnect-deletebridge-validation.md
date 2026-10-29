Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 68c868d69b495651e8ae949e9225c232ca965615

# DeleteBridge drops a bridge that a flow source still names, leaving GatewayBridgeSource.BridgeArn pointing at nothing — the mirror case DeleteFlow refuses

_Filed: 2026-10-29T06:21:03Z_
_Operation: DeleteBridge | Class: validation_

> _Note: this issue was identified against the code base at commit `68c868d69b495651e8ae949e9225c232ca965615`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A flow source can take its content from a bridge: `Source$GatewayBridgeSource` carries `BridgeArn`, which the model makes REQUIRED, and all three paths that write such a source resolve that ARN against `store.bridges` before storing it (CreateFlow's `Source`/`Sources`, AddFlowSources, UpdateFlowSource), so the crate guarantees "a stored source never names a bridge the account has not got".

DeleteBridge removes the bridge record outright with no regard for those references: it checks only that the bridge is stopped, then calls `store.bridges.remove(..)`. It neither refuses the delete while a flow source names the bridge nor repairs the references afterwards, so after the call a stored flow source keeps `GatewayBridgeSource.BridgeArn` naming a bridge the store no longer holds — precisely the state the crate says must not arise, and one no later call can restate (every write path refuses that ARN now) nor clear (the member is `required`, so it cannot be dropped off the record the way an optional router-configuration reference is).

The crate already implements both halves of this rule for the MIRROR relation and for the router resources:
* DeleteFlow REFUSES while a bridge's flow source names the flow (`flow_bridge_source_rejection`), with the reason stated as: a required member cannot be dropped off the record, so taking the target away is refused instead;
* DeleteFlow / RemoveFlowOutput / RemoveFlowSource CLEAR the optional router references (`clear_unresolvable_flow_references`), and `settle_due_transitions` clears a router output's route into a router input the clock removed.

DeleteBridge does neither, and it is the only removal in this crate that takes away the target of a REQUIRED stored reference.

## Evidence

The rule the crate states for this very reference — services/service-com-amazonaws-mediaconnect-mediaconnect/src/state.rs:5890-5905 (`source_bridge_reference_rejection`):
```
    /// `GatewayBridgeSource$BridgeArn` is "the ARN of the bridge feeding this flow", so it
    /// is a reference to a bridge of the ACCOUNT ... Every operation that writes one
    /// (CreateFlow's `Source` / `Sources`, AddFlowSources, UpdateFlowSource) resolves it
    /// HERE, so a stored source never names a bridge the account has not got
```
and what a deleted bridge means to that lookup — src/state.rs:5883-5888:
```
    /// Whether the store holds the bridge `arn` ... A bridge leaves the store when DeleteBridge takes it out, so a
    /// reference to a deleted bridge resolves nothing.
    pub fn holds_bridge(&self, arn: &str) -> bool {
```
The member is REQUIRED, so it cannot be dropped off a stored source — smithy_query(query="shape",shape="GatewayBridgeSource"):
```
"BridgeArn": {"target": "String", "jsonName": "bridgeArn", "clientOptional": true, "required": true, "doc": "<p> The ARN of the bridge feeding this flow.</p>"}
```
DeleteBridge removes the record with no reference check and no repair — src/ops/delete_bridge.rs:198-213:
```
        // The record is dropped rather than retired: `DeleteBridgeResponse` reports only
        // the ARN of the deleted bridge ...
        let Some(deleted) = store.bridges.remove(&input.bridge_arn) else {
...
        Ok(DeleteBridgeOutput {
            bridge_arn: Some(deleted.bridge_arn),
        })
```
The MIRROR removal in the same crate does apply the rule — src/ops/delete_flow.rs:209-229:
```
        // A bridge's flow source names this flow in a REQUIRED member, resolved against this
        // store by every path that writes such a source, so a reference that outlived the
        // flow could not be dropped off the bridge and no later call could restate it. The
        // delete is refused while one exists ...
        if let Some(detail) = store.flow_bridge_source_rejection(&flow_arn) {
...
        store.clear_unresolvable_flow_references(now);
```
and state.rs:6226-6239 states it in general terms:
```
    /// ... Taking the flow away while a bridge still names it
    /// would therefore leave that bridge holding a reference nothing resolves, which no later
    /// call could even restate; and unlike a router input's or a router output's connection to
    /// a flow, which the model makes optional and which a removal therefore DROPS
    /// ([`Self::clear_unresolvable_flow_references`]), a required member cannot be dropped off
    /// the record. So the removal is refused while a bridge source names the flow
```
The three write paths that refuse the dangling ARN: src/ops/create_flow.rs:473 `store.source_bridge_reference_rejection(&format!("Sources[{index}]"), ...)`, src/ops/add_flow_sources.rs:281 (same call), src/ops/update_flow_source.rs:483 (same call).

Test coverage: the suite pins the write side against exactly this ARN and nothing pins the delete side — tests/all/flow_source_bridge_and_ingest_port.rs:181-184:
```
        UnresolvableBridge {
            case: "the ARN of a bridge DeleteBridge removed".to_string(),
            arn: removed.clone(),
        },
```
(the rows are then asserted refused on CreateFlow, AddFlowSources and UpdateFlowSource at :202, :224, :240). No test deletes a bridge a stored flow source names.

## Steps to Reproduce

1. POST /v1/gateways with {"name":"gw","egressCidrBlocks":["10.0.0.0/16"],"networks":[{"name":"net-1","cidrBlock":"10.0.1.0/24"}]} — keep GatewayArn.
2. POST /v1/bridges with {"name":"br","placementArn":"<GatewayArn>","ingressGatewayBridge":{"maxBitrate":20000000,"maxOutputs":2},"sources":[{"networkSource":{"name":"s1","networkName":"net-1","protocol":"rtp","port":5000,"multicastIp":"224.0.0.1"}}]} — keep BridgeArn.
3. POST /v1/flows with {"name":"f1","source":{"name":"src","gatewayBridgeSource":{"bridgeArn":"<BridgeArn>"}}} — 201; keep FlowArn and the source's SourceArn.
4. Wait one transition span so the bridge settles into STANDBY, then DELETE /v1/bridges/<BridgeArn> — 200, and the bridge record is gone (no refusal mentioning the flow that names it).
5. GET /v1/flows/<FlowArn> — the stored source still reports gatewayBridgeSource.bridgeArn = <BridgeArn>, a bridge the store no longer holds.
6. PUT /v1/flows/<FlowArn>/source/<SourceArn> with {"gatewayBridgeSource":{"bridgeArn":"<BridgeArn>"}} — 400 BadRequestException "GatewayBridgeSource.BridgeArn ... does not name a bridge this account holds": the stored state cannot be restated, and because the member is required it cannot be cleared off the source either.
