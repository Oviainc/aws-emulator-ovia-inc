Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a19f71fb9b9d334f8efc64d442ea8394bd20139d

# AddBridgeOutputs stores a network output whose NetworkName the placement gateway does not declare, which UpdateBridgeOutput refuses

_Filed: 2026-10-28T15:07:30Z_
_Operation: AddBridgeOutputs | Class: validation_

> _Note: this issue was identified against the code base at commit `a19f71fb9b9d334f8efc64d442ea8394bd20139d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AddBridgeOutputs` materialises each requested network output and checks only its name validity, name uniqueness and the per-bridge output bound. It never resolves `BridgeNetworkOutput$NetworkName` against the networks declared by the gateway the bridge is placed on. `UpdateBridgeOutput`, writing the same stored shape on the same bridge, resolves exactly that name and refuses it with `BadRequestException`.

The result is that a network name is accepted by the add path and refused by the update path on the same bridge, and the bridge is left holding an output delivered on a network its placement gateway does not declare.

## Evidence

UpdateBridgeOutput resolves the name — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/update_bridge_output.rs:283-295:
```
            // "The network output's gateway network name"
            // (`BridgeNetworkOutput$NetworkName`): a network output is delivered on one of
            // the networks the gateway this bridge is placed on declares, so the name is
            // resolved against that gateway's own networks.
            if let Some(network_name) = update.network_name.as_deref() {
                if let Some(rejection) = store.unknown_gateway_network_rejection(
                    "NetworkOutput NetworkName",
                    &placement_arn,
                    network_name,
                ) {
                    return Err(bad_request(rejection));
                }
            }
```
AddBridgeOutputs' whole per-entry loop, with no such resolution — src/ops/add_bridge_outputs.rs:241-259:
```
        for request in &input.outputs {
            let Some(output) = crate::state::materialise_bridge_output(request) else { ... };
            let name = crate::state::bridge_output_name(&output);
            if !crate::state::is_valid_bridge_member_name(name) { ... }
            if !taken.insert(name.to_string()) { ... }
            added.push(output);
        }
```
The rule — src/state.rs:5664-5672: "`BridgeNetworkOutput$NetworkName` (\"the network output's gateway network name\") references a network of the ONE gateway the bridge is placed on (`Bridge$PlacementArn`) ... and a name outside it references nothing".

Test coverage: tests/all/bridge_update_references.rs:426 (`a_network_output_resolves_its_network_name_on_the_same_placement_gateway`) pins the rule for UpdateBridgeOutput; tests/all/bridge_output_admission.rs asserts only the direction/count rules for the add path, stating no reason for accepting an unresolvable network name.

## Steps to Reproduce

1. POST /v1/gateways to create gateway G declaring network "net-1".
2. POST /v1/bridges to create an egress (cloud-to-ground) bridge placed on G with one flow source.
3. POST /v1/bridges/{BridgeArn}/outputs with `{"outputs":[{"networkOutput":{"name":"out-1","networkName":"no-such-net","ipAddress":"10.0.0.5","port":6000,"protocol":"rtp","ttl":10}}]}` — observe 202 and DescribeBridge reporting the output on "no-such-net".
4. PUT /v1/bridges/{BridgeArn}/outputs/out-1 with `{"networkOutput":{"networkName":"no-such-net"}}` — observe 400 BadRequestException for the value step 3 already stored.
