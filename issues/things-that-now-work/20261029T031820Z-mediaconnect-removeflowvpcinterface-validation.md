Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c0c2239955f7987f5942ca86a100c431d5defb9f

# RemoveFlowVpcInterface ignores the VPC interface a media stream's destination/input configuration names, so an output keeps an Interface.Name the flow no longer holds

_Filed: 2026-10-29T03:18:20Z_
_Operation: RemoveFlowVpcInterface | Class: validation_

> _Note: this issue was identified against the code base at commit `c0c2239955f7987f5942ca86a100c431d5defb9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

RemoveFlowVpcInterface resolves the documented "referenced by a Source or Output" precondition through `StoredFlow::vpc_interface_reference`, which looks at exactly three places: `Source.VpcInterfaceName`, `Source.GatewayBridgeSource.VpcInterfaceAttachment.VpcInterfaceName` and `Output.VpcInterfaceAttachment.VpcInterfaceName`.

It does NOT look at the fourth place a stored output or source names a VPC interface by name: `Output.MediaStreamOutputConfigurations[*].DestinationConfigurations[*].Interface.Name` and `Source.MediaStreamSourceConfigurations[*].InputConfigurations[*].Interface.Name`. Those names are stored verbatim (`materialise_media_stream_source_configurations`, state.rs:1690) and the crate's OWN write paths resolve them against the flow's VPC interfaces before storing them (`media_stream_output_configuration_rejection` / `media_stream_source_configuration_rejection`, which call `unknown_vpc_interface_rejection`).

So the removal is accepted while an output of the same flow still carries that interface name in its media stream destination configuration, and the flow is left holding a configuration that the crate's own CreateFlow / AddFlowOutputs / AddFlowSources / UpdateFlowOutput / UpdateFlowSource paths refuse to store and that no later call could restate.

## Evidence

Model (smithy_query doc="RemoveFlowVpcInterface"): "Removes a VPC Interface from an existing flow. This request can be made only on a VPC interface that does not have a Source or Output associated with it. If the VPC interface is referenced by a Source or Output, you must first delete or update the Source or Output to no longer reference the VPC interface."

Model (smithy_query shape="Interface"): member `Name` — "The name of the VPC interface."; shape doc — "The VPC interface that is used for the media stream associated with the source or output."

Handler, the only reference check it makes — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/remove_flow_vpc_interface.rs:228:
```
        if let Some(reference) = flow.vpc_interface_reference(&input.vpc_interface_name) {
```

What that helper resolves — src/state.rs:2562-2583 (only `source.vpc_interface_name`, the gateway-bridge attachment, and `output.vpc_interface_attachment`):
```
    pub fn vpc_interface_reference(&self, vpc_interface_name: &str) -> Option<String> {
        let source = self.sources.iter().find(|source| {
            source.vpc_interface_name.as_deref() == Some(vpc_interface_name)
                || source
                    .gateway_bridge_source
                    ... .vpc_interface_name.as_deref()
                    == Some(vpc_interface_name)
        });
```

The SAME crate resolves the destination/input-configuration reference on every write — src/state.rs:2266-2270:
```
                    unknown_vpc_interface_rejection(
                        &format!("{entry}.DestinationConfigurations[{position}].Interface.Name"),
                        &configuration.interface.name,
                        vpc_interfaces,
                    )
```
and src/state.rs:2303-2307 for `InputConfigurations[{position}].Interface.Name`; `unknown_vpc_interface_rejection` (state.rs:2221) answers "{member} {name:?} does not name a VPC interface this flow holds."

Test coverage: services/service-com-amazonaws-mediaconnect-mediaconnect/tests/all/remove_flow_vpc_interface.rs `reference_cases()` (lines 460-507) pins exactly three rows — "a source that ingests through the VPC interface", "an output attached to the VPC interface", "a source fed by a gateway bridge over the VPC interface". No row carries a `mediaStreamOutputConfigurations[*].destinationConfigurations[*].interface` reference, so the gap is untested.

## Steps to Reproduce

1. CreateFlow with `vpcInterfaces: [{name: "vpc-ref", ...}]`, one media stream `ms-1`, and one output whose `mediaStreamOutputConfigurations: [{mediaStreamName: "ms-1", destinationConfigurations: [{destinationIp, destinationPort, interface: {name: "vpc-ref"}}]}]` (accepted, because the name resolves).
2. DELETE /v1/flows/{FlowArn}/vpcInterfaces/vpc-ref — observe 200 and the interface removed, instead of the documented BadRequestException for a VPC interface an Output references.
3. DescribeFlow: the output still reports `destinationConfigurations[0].interface.name == "vpc-ref"`, while `vpcInterfaces` no longer holds it.
4. Try to restate that same output through UpdateFlowOutput with the same `mediaStreamOutputConfigurations` — it is refused 400 "...Interface.Name \"vpc-ref\" does not name a VPC interface this flow holds.", proving the stored record is one the crate's own write paths reject.
