Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# CreateRouterOutput stores a Configuration whose referenced network interface / flow / MediaLive input does not exist

_Filed: 2026-10-24T13:28:20Z_
_Operation: CreateRouterOutput | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Configuration` is a REQUIRED member of CreateRouterOutput and each variant of `RouterOutputConfiguration` names a target: `Standard.NetworkInterfaceArn` is a `routerNetworkInterface` ARN of THIS service (required), `MediaConnectFlow` names a flow (and flow source) of THIS service, `MediaLiveInput` names a MediaLive input.

Expected: a create whose configuration names a router network interface or a MediaConnect flow the account does not hold is refused with HTTP 400 BadRequestException (modelled for this operation) and no router output is stored — the target is what the output egresses to, so AWS cannot create an output pointing at nothing.

Actual: the handler normalises the configuration union only for internal protocol/variant consistency and stores it verbatim; its own comment states the omission ("No variant's target is RESOLVED here … referential checks belong with the operations that own those resources"). A router output naming a fabricated `routerNetworkInterface:…` ARN, or a flow ARN that was never created, is accepted with HTTP 201 and reported by GetRouterOutput/BatchGetRouterOutput as a real output. The same hole exists on the update path: UpdateRouterOutput re-normalises a replacement configuration (`src/ops/update_router_output.rs:261-286`) and never resolves the new target either.

## Evidence

Spec: `smithy_query(query="shape",shape="StandardRouterOutputConfiguration")` — `NetworkInterfaceArn` required, pattern `^arn:…:routerNetworkInterface:[a-z0-9]{12}$`, i.e. a resource this service itself mints; `MediaConnectFlowRouterOutputConfiguration` names a flow of this service. `smithy_query(query="operation",operation="CreateRouterOutput")` models BadRequestException (400).

Handler: `src/ops/create_router_output.rs:245-257` — the comment explicitly declines the referential check and `crate::state::normalized_router_output_configuration` only reconciles the declared `Protocol` with the chosen variant; nothing consults `store.router_network_interfaces` or `store.flows` before the insert at lines 320-370.

Test coverage: the current leniency is pinned — `tests/all/router_outputs.rs:31-51` defines hand-written `NETWORK_INTERFACE_ARN`, `FLOW_ARN`, `FLOW_SOURCE_ARN` and `MEDIALIVE_INPUT_ARN` constants for resources that were never created, and the create cases at lines 224-240 expect success with them.

## Steps to Reproduce

1. Boot the emulator with no router network interfaces and no flows
2. POST /v1/routerOutput with {"name":"ro-1","tier":"HD","maximumBitrate":20000000,"routingScope":"REGIONAL","configuration":{"standard":{"networkInterfaceArn":"arn:aws:mediaconnect:us-east-1:000000000000:routerNetworkInterface:a1b2c3d4e5f6","protocolConfiguration":{...}}}}
3. Observe HTTP 201, where AWS answers 400 BadRequestException for the unknown network interface
4. POST /v1/routerOutput with {"configuration":{"mediaConnectFlow":{"flowArn":"arn:aws:mediaconnect:us-east-1:000000000000:flow:1-0000000a:nope","flowSourceArn":"…:source:1-0000000a:nope-src"}}} — again 201 for a flow that does not exist
5. PUT /v1/routerOutput/{Arn} with a replacement `standard` configuration naming another non-existent interface — accepted 202
