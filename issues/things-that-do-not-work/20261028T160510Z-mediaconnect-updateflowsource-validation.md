Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5f1c460e5544105d602e680f733bf81495110ec8

# UpdateFlowSource stores a GatewayBridgeSource whose BridgeArn names no bridge, and an IngestPort the Zixi rule forbids

_Filed: 2026-10-28T16:05:10Z_
_Operation: UpdateFlowSource | Class: validation_

> _Note: this issue was identified against the code base at commit `5f1c460e5544105d602e680f733bf81495110ec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two client-supplied members of UpdateFlowSource are written to the store with no check, although the handler checks every neighbouring member of the same request:

1. `GatewayBridgeSource.BridgeArn` is taken on trust. The handler resolves the VPC interface attachment OF THAT VERY MEMBER against the flow's interfaces (`vpc_interface_attachment_rejection`, "GatewayBridgeSource.VpcInterfaceAttachment"), but never resolves the bridge ARN against `store.bridges`, even though the crate resolves every other cross-resource reference it stores (`unknown_flow_rejection` for a bridge flow source's FlowArn in UpdateBridgeSource, `unknown_gateway_network_rejection`, `unknown_router_network_interface_rejection`, `unknown_flow_output_rejection`). A source is left naming a bridge the account does not hold, which DescribeFlow then reports. The same reference is stored unresolved by CreateFlow / AddFlowSources through `source_request_reference_rejection`, which resolves only the attachment (src/state.rs:2315-2349).

2. `IngestPort` is stored as given even when the source's protocol is Zixi, which the model's own sentence forbids.

## Evidence

The model states the port rule itself — smithy_query(query="doc",shape="UpdateFlowSourceRequest",member="IngestPort"): "The port that the flow listens on for incoming content. If the protocol of the source is Zixi, the port must be set to 2088." (also quoted in the handler's own header, src/ops/update_flow_source.rs:91). The handler stores any value: src/ops/update_flow_source.rs:476-478:
```
        if let Some(ingest_port) = input.ingest_port {
            updated.ingest_port = Some(ingest_port);
        }
```
(no occurrence of `2088` anywhere under src/ — verified by grep.)

The bridge ARN is merged and written unresolved — src/ops/update_flow_source.rs:290-299 and :518-527:
```
    let bridge_arn = update
        .bridge_arn
        .clone()
        .or_else(|| stored.map(|bridge| bridge.bridge_arn.clone()))?;
    merged.bridge_arn = bridge_arn;
...
        if let Some(update) = input.gateway_bridge_source.as_ref() {
            ...
            updated.gateway_bridge_source = Some(bridge_source);
```
while the SAME handler resolves the attachment of that member — src/ops/update_flow_source.rs:460-469:
```
        if let Some(rejection) = crate::state::vpc_interface_attachment_rejection(
            "GatewayBridgeSource.VpcInterfaceAttachment",
            input
                .gateway_bridge_source
                .as_ref()
                .and_then(|bridge_source| bridge_source.vpc_interface_attachment.as_ref()),
            &held_vpc_interfaces,
```
The crate's own rule for a stored ARN reference — src/ops/update_bridge_source.rs:296:
```
                if let Some(rejection) = store.unknown_flow_rejection("FlowSource FlowArn", flow_arn)
```
and src/state.rs:5649-5657 (`unknown_flow_rejection`: "Why `arn` names no flow this account holds").

Test coverage: tests/all/update_flow_source.rs:270-283 currently accepts an invented bridge ARN ("arn:aws:mediaconnect:us-east-1:111122223333:bridge:1-0000cafe:br-1") with the stated reason being only that "`GatewayBridgeSource$BridgeArn` is required on the stored shape, so this row names it" — no reason is given for admitting an unresolvable bridge; that row (and tests/all/remove_flow_vpc_interface.rs:497) would need a real bridge ARN. No test combines a Zixi protocol with a non-2088 `ingestPort`.

## Steps to Reproduce

1. POST /v1/flows with a source {"name":"src","protocol":"rtp","whitelistCidr":"10.0.0.0/16"} — 201; keep FlowArn and SourceArn.
2. PUT /v1/flows/{FlowArn}/source/{SourceArn} with {"gatewayBridgeSource":{"bridgeArn":"arn:aws:mediaconnect:us-east-1:111122223333:bridge:1-0000cafe:br-1"}} while no bridge exists — observe 202 and GET /v1/flows/{FlowArn} reporting the source with that bridge ARN; GET /v1/bridges/{that ARN} answers 404.
3. Compare: PUT /v1/bridges/{BridgeArn}/sources/{name} with a flowSource naming an unknown flow is refused 400 "FlowSource FlowArn … does not name a flow this account holds."
4. PUT /v1/flows/{FlowArn}/source/{SourceArn} with {"protocol":"zixi-push","ingestPort":5101} — observe 202 and the stored source reporting protocol zixi-push with ingestPort 5101, contradicting "If the protocol of the source is Zixi, the port must be set to 2088."
