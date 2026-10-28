Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7cfd583752b909e6f589468ac42594db7887f86f

# CreateBridge stores a PlacementArn that names no gateway of the account, so the bridge is placed on a gateway the store does not hold

_Filed: 2026-10-28T19:03:07Z_
_Operation: CreateBridge | Class: validation_

> _Note: this issue was identified against the code base at commit `7cfd583752b909e6f589468ac42594db7887f86f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateBridgeRequest$PlacementArn` is the gateway the bridge is placed on (`Bridge$PlacementArn`, `ListedBridge$PlacementArn`: "the ARN of the gateway associated with the bridge"). CreateBridge copies it onto the stored record without ever resolving it against `store.gateways`, so a bridge can be created on an ARN that names no gateway at all — a state the rest of the crate cannot work with:

* `unknown_gateway_network_rejection` resolves a bridge network name against "the placement gateway's own StoredGateway::networks" and documents that a store holding no gateway for `placement_arn` "declares no network at all" — so on such a bridge UpdateBridgeOutput and UpdateBridgeSource must refuse EVERY network name, and no network output or network source of it can ever be updated;
* DeleteGateway's and DeregisterGatewayInstance's preconditions count the bridges placed on a gateway by this same `placement_arn`, so a bridge whose placement names nothing is held against no gateway.

Every other cross-resource reference this crate stores is resolved against the store rather than taken on trust — UpdateBridgeSource resolves its own `FlowSource FlowArn` through `unknown_flow_rejection` and its `NetworkSource NetworkName` through `unknown_gateway_network_rejection`, and the router creates resolve their interface and flow references the same way — and the stated reason is that storing an unresolvable reference leaves a record pointing at nothing.

## Evidence

CreateBridge stores the placement ARN unresolved; nothing in the handler reads `store.gateways` — src/ops/create_bridge.rs:374-379:
```
        store.bridges.insert(
            bridge_arn.clone(),
            StoredBridge {
                bridge_arn: bridge_arn.clone(),
                name: input.name.clone(),
                placement_arn: input.placement_arn.clone(),
```
The crate's own reading of a placement ARN that resolves to no gateway — src/state.rs:5700-5720:
```
    /// A bridge member that carries a gateway network name — `BridgeNetworkSource$NetworkName`
    /// ("the network source's gateway network name") ... references a network of the ONE gateway
    /// the bridge is placed on (`Bridge$PlacementArn`) ...
    /// The valid set is therefore the placement gateway's own [`StoredGateway::networks`],
    /// and a name outside it references nothing — including every name when the store holds no
    /// gateway for `placement_arn`, which declares no network at all.
    pub fn unknown_gateway_network_rejection(
```
used by the sibling update paths — src/ops/update_bridge_output.rs:287-294 and src/ops/update_bridge_source.rs:296, :332:
```
            if let Some(network_name) = update.network_name.as_deref() {
                if let Some(rejection) = store.unknown_gateway_network_rejection(
                    "NetworkOutput NetworkName",
                    &placement_arn,
                    network_name,
...
                if let Some(rejection) = store.unknown_flow_rejection("FlowSource FlowArn", flow_arn)
```
and the rule those lookups state — src/state.rs:5597-5602:
```
    /// A `RouterNetworkInterfaceArn` names a resource THIS service mints
    /// ... so a request that points at one is resolved
    /// against the store rather than taken on trust: an ARN of the right shape for an
    /// interface the account never created names nothing.
```
Test coverage: tests/all/bridge_lifecycle.rs and tests/all/create_bridge_direction_contract.rs always create the gateway first and pass its ARN; no test states that a placement ARN naming no gateway is acceptable.

## Steps to Reproduce

1. POST /v1/bridges with {"name":"b1","placementArn":"arn:aws:mediaconnect:us-east-1:123456789012:gateway:1-abcdefgh-not-created","sources":[{"networkSource":{"name":"s1","networkName":"net-1","protocol":"rtp","port":5000,"multicastIp":"224.0.0.1"}}],"ingressGatewayBridge":{"maxBitrate":80000000,"maxOutputs":2}} — observe 201 and a stored bridge whose placementArn names no gateway.
2. GET /v1/bridges/{BridgeArn} — observe placementArn reported back as a gateway the account never created.
3. PUT /v1/bridges/{BridgeArn}/sources/s1 with {"networkSource":{"networkName":"net-1"}} — observe 400 BadRequestException "NetworkSource NetworkName ... does not name a network this gateway declares": no network name can ever be set on this bridge.
4. Confirm the bridge is held against no gateway: DELETE /v1/gateways/{AnyRealGatewayArn} is never blocked by it, because the placement precondition matches bridges by placementArn.
