Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# CreateRouterInput stores a Configuration whose referenced router network interface / flow does not exist

_Filed: 2026-10-24T13:28:20Z_
_Operation: CreateRouterInput | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Configuration` is a REQUIRED member of CreateRouterInput and every variant of `RouterInputConfiguration` names another resource by ARN: `Standard.NetworkInterfaceArn` is a `routerNetworkInterface` ARN of THIS service ("the ARN of the network interface associated with the standard router input", required), `MediaConnectFlow` names a flow (and its source) of THIS service, `Failover`/`Merge` name the inputs they combine, `MediaLiveChannel` names a MediaLive channel.

Expected: a create whose configuration names a router network interface (or flow/source) the account does not hold is refused with HTTP 400 BadRequestException — the client error CreateRouterInput models — and no router input is stored. AWS will not create an input bound to an interface that does not exist, because the interface is what the input ingests on.

Actual: the handler validates only the bitrate/tier pair, the Region and Availability Zone tokens, the tag pairs and the store ceiling, then stores `input.configuration.clone()` verbatim. No lookup against `store.router_network_interfaces` (or `store.flows`) is ever made, so a router input referencing a fabricated `routerNetworkInterface:a1b2c3d4e5f6` ARN is created with HTTP 201 and is then reported by GetRouterInput/RestartRouterInput as a healthy input. The dangling reference is also invisible on the other side: `router_network_interface_projection` counts associations from inputs/outputs that name an interface, so the association can never be read back from the interface the input claims.

## Evidence

Spec: `smithy_query(query="shape",shape="StandardRouterInputConfiguration")` — `NetworkInterfaceArn` is required and constrained to the `^arn:…:routerNetworkInterface:[a-z0-9]{12}$` pattern this same service mints in CreateRouterNetworkInterface; `smithy_query(query="operation",operation="CreateRouterInput")` models BadRequestException (400), so the refusal has a modelled channel.

Handler: `src/ops/create_router_input.rs:194-406` — checks `maximum_bitrate`, `router_input_tier_bitrate_ceiling`, `is_region_token`, `is_zone_of_region`, tag bounds, `MAX_ROUTER_INPUTS` and the ClientToken replay, then line 341 `let configuration = input.configuration.clone();` is stored unchanged. There is no reference to `router_network_interfaces` or `flows` anywhere in the file.

Test coverage: the suite PINS the lenient behaviour — `tests/all/router_input_restart.rs:66,142-203` creates router inputs whose `standard.networkInterfaceArn` is the hand-written constant ARN (see `tests/all/router_outputs.rs:31-35` for the same fabricated `routerNetworkInterface:a1b2c3d4e5f6`), an ARN the store never minted, and expects 201.

## Steps to Reproduce

1. POST /v1/routerNetworkInterface is NOT called, so the account holds no router network interface
2. POST /v1/routerInput with {"name":"ri-1","tier":"HD","maximumBitrate":20000000,"routingScope":"REGIONAL","configuration":{"standard":{"networkInterfaceArn":"arn:aws:mediaconnect:us-east-1:000000000000:routerNetworkInterface:a1b2c3d4e5f6","protocolConfiguration":{...}}}}
3. Observe HTTP 201 with a stored router input, where AWS answers 400 BadRequestException because the network interface does not exist
4. GET /v1/routerInput/{Arn} — the input is reported bound to an interface no BatchGetRouterNetworkInterface call can resolve
5. Repeat with {"configuration":{"mediaConnectFlow":{"flowArn":"arn:aws:mediaconnect:us-east-1:000000000000:flow:1-0000000a:nope",...}}} — same unconditional acceptance
