Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a19f71fb9b9d334f8efc64d442ea8394bd20139d

# AddBridgeSources stores a flow source whose FlowArn resolves to no flow and a network source whose NetworkName the placement gateway does not declare, both of which UpdateBridgeSource refuses

_Filed: 2026-10-28T15:07:30Z_
_Operation: AddBridgeSources | Class: validation_

> _Note: this issue was identified against the code base at commit `a19f71fb9b9d334f8efc64d442ea8394bd20139d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AddBridgeSources` materialises each requested source and checks only its name (non-empty, no '/'), name uniqueness, and the bridge-direction/count rules. It never resolves the two references a bridge source carries: `BridgeFlowSource$FlowArn` against the account's flows, and `BridgeNetworkSource$NetworkName` against the networks of the gateway the bridge is placed on. `UpdateBridgeSource`, writing the very same stored shape on the very same bridge, resolves both and refuses with `BadRequestException`.

So the same value is accepted on the add path and refused on the update path: a bridge can be left holding a flow source pointing at a flow this account never minted, or a network source ingesting on a network name its placement gateway does not declare — stored state the update path would never permit.

## Evidence

UpdateBridgeSource resolves both references — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/update_bridge_source.rs:290-300:
```
            // "The ARN of the cloud flow used as a source of this bridge"
            // (`BridgeFlowSource$FlowArn`): a flow source originates "in MediaConnect as an
            // existing cloud flow", so the ARN is RESOLVED against the account's flows
            // through the one function every operation of this service resolves a flow
            // reference through.
            if let Some(flow_arn) = update.flow_arn.as_deref() {
                if let Some(rejection) = store.unknown_flow_rejection("FlowSource FlowArn", flow_arn)
```
and update_bridge_source.rs:327-338:
```
            // "The network source's gateway network name"
            // (`BridgeNetworkSource$NetworkName`): a network source ingests on one of the
            // networks the gateway this bridge is placed on declares, so the name is
            // resolved against that gateway's own networks.
            if let Some(network_name) = update.network_name.as_deref() {
                if let Some(rejection) = store.unknown_gateway_network_rejection(
```
AddBridgeSources' whole per-entry loop, with no reference resolution — src/ops/add_bridge_sources.rs:229-253:
```
        for request in &input.sources {
            let Some(source) = crate::state::materialise_bridge_source(request) else { ... };
            let name = crate::state::bridge_source_name(&source);
            if !crate::state::is_valid_bridge_member_name(name) { ... }
            if !taken.insert(name.to_string()) { ... }
            if let Some(detail) = crate::state::bridge_source_kind_rejection(bridge_type, &source) { ... }
            added.push(source);
        }
```
(`store.unknown_flow_rejection` / `store.unknown_gateway_network_rejection` are reached from nowhere else than update_bridge_source.rs and update_bridge_output.rs — grep of src/ops.)

The rule the crate states for these members — src/state.rs:5660-5674:
```
    /// A bridge member that carries a gateway network name — `BridgeNetworkSource$NetworkName`
    /// ("the network source's gateway network name") ... references a network of the ONE gateway
    /// the bridge is placed on (`Bridge$PlacementArn`) ... The valid set is therefore the
    /// placement gateway's own [`StoredGateway::networks`], and a name outside it references
    /// nothing
```
Test coverage: tests/all/bridge_update_references.rs:7-16 pins the rule for the UPDATE path only ("The rule under test: a bridge member that names another resource is RESOLVED against the store before it is written"); tests/all/bridge_source_admission.rs:62 happens to feed `minted_flow_arn(...)` for a flow it never creates while asserting only the direction/count rules its module doc states, so no test states a reason for accepting an unresolvable reference on the add path.

## Steps to Reproduce

1. POST /v1/gateways to create gateway G declaring network "net-1".
2. POST /v1/bridges to create an ingress bridge placed on G with one network source on "net-1".
3. POST /v1/bridges/{BridgeArn}/sources with `{"sources":[{"networkSource":{"name":"src-2","networkName":"no-such-net","multicastIp":"224.0.0.1","port":6000,"protocol":"rtp"}}]}` — observe 202 and DescribeBridge reporting a source on "no-such-net".
4. PUT /v1/bridges/{BridgeArn}/sources/src-2 with `{"networkSource":{"networkName":"no-such-net"}}` — observe 400 BadRequestException "NetworkSource NetworkName \"no-such-net\" does not name a network the gateway ... the bridge is placed on declares." for the value step 3 already stored.
5. Repeat for an egress bridge with `{"sources":[{"flowSource":{"name":"f-2","flowArn":"arn:aws:mediaconnect:<region>:111122223333:flow:1-0000dead:never-minted"}}]}` and compare with the same value through PUT .../sources/f-2.
