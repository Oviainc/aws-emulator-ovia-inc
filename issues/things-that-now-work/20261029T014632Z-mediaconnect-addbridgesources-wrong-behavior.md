Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1211f7611505016a47ab1bf8932fdc778b7fd399

# A bridge source taken from a cloud flow never gets the flow output MediaConnect creates for it, so BridgeFlowSource.OutputArn is always absent

_Filed: 2026-10-29T01:46:32Z_
_Operation: AddBridgeSources | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1211f7611505016a47ab1bf8932fdc778b7fd399`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AddBridgeSources (and CreateBridge, through the same materialisation) stores a `FlowSource` entry by copying the request's `FlowArn`, name and VPC interface attachment, and hard-codes `OutputArn: None`. The crate's own comment on that line states what the member means — "the Amazon Resource Number (ARN) of the output MediaConnect creates on the cloud flow for this bridge" — and then admits the emulator does not do it: "This emulator creates no output on the flow a bridge names".

So after a successful AddBridgeSources naming a cloud flow:

* DescribeBridge reports the bridge's flow source with `OutputArn` absent, i.e. the bridge is fed by an output that does not exist; and
* DescribeFlow on the named source flow reports no output at all for the bridge, so nothing in the store records that the flow is feeding the bridge.

This is not an absent feature of the crate: the crate already owns the whole "mint an output on a flow" materialisation (`StoredOutput::from_add_request`, which "mints the `OutputArn`"), and it already keeps the stored source record that would carry the result. The one thing missing is the call, which the comment says is deliberately skipped. UpdateBridgeSource then carries the absent `OutputArn` over untouched even when the request repoints the source at a different flow, so no later call can ever fill it in either.

## Evidence

The admitted gap, src/state.rs:2938-2951 (`materialise_bridge_source`, the crate's ONE materialisation of a bridge source, called by CreateBridge and AddBridgeSources):
```
        (Some(flow_source), None) => Some(BridgeSource {
            flow_source: Some(BridgeFlowSource {
                flow_arn: flow_source.flow_arn.clone(),
                flow_vpc_interface_attachment: flow_source.flow_vpc_interface_attachment.clone(),
                name: flow_source.name.clone(),
                // `BridgeFlowSource$OutputArn` is "the Amazon Resource Number (ARN) of the
                // output" MediaConnect creates on the cloud flow for this bridge. This
                // emulator creates no output on the flow a bridge names — the flow's
                // outputs are what AddFlowOutputs / RemoveFlowOutput own — so the optional
                // member stays absent rather than carrying an ARN that addresses nothing.
                output_arn: None,
            }),
```
The call site that reaches it, src/ops/add_bridge_sources.rs:233:
```
            let Some(source) = crate::state::materialise_bridge_source(request) else {
```
The machinery the crate already has for exactly this write, src/state.rs:1735-1755:
```
    /// Materialise one `AddOutputRequest` into the output record a flow keeps.
    ///
    /// Every operation that adds an output to a flow (CreateFlow's `Outputs`,
    /// AddFlowOutputs) materialises it HERE, so one request shape always becomes the
    /// same stored record: the service mints the `OutputArn`, ...
    pub fn from_add_request(
...
            output_arn: ::engine::emulator_arn(
                SERVICE_NAMESPACE,
                region,
                &output_resource_fragment(suffix, &name),
            ),
```
The member the model declares for the stored source, `smithy_query(query="doc",shape="BridgeFlowSource",member="OutputArn")`: "The Amazon Resource Number (ARN) of the output."

Test coverage: tests/all/bridge_source_admission.rs and tests/all/bridge_add_references.rs exercise AddBridgeSources' admission and reference resolution; tests/all/update_bridge_source.rs:11 pins only that the stored `OutputArn` "is left exactly as it was" across an update. A repo-wide grep for `outputArn` in tests/ finds no assertion that a bridge flow source's `OutputArn` is absent, so nothing pins today's behaviour.

## Steps to Reproduce

1. POST /v1/flows to create a flow — 201; keep FlowArn.
2. POST /v1/gateways to create a gateway — 201; keep GatewayArn; wait one transition span.
3. POST /v1/bridges with a PlacementArn of that gateway and an egress (cloud-to-ground) direction — 201; keep BridgeArn.
4. POST /v1/bridges/{BridgeArn}/sources with {"sources":[{"flowSource":{"name":"from-flow","flowArn":"<FlowArn>"}}]} — observe 201.
5. GET /v1/bridges/{BridgeArn} — observe the source reported with no `outputArn`.
6. GET /v1/flows/{FlowArn} — observe `outputs` unchanged: no output was created on the flow that is supposed to be feeding the bridge.
