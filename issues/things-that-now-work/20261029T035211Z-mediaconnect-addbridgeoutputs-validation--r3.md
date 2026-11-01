Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 50acf5c50341569d447ddf04c6c32ab03f586e64

# AddBridgeOutputs adds network outputs to a bridge retired in the terminal DELETED state, which the crate's own UpdateBridgeState calls irreversible

_Filed: 2026-10-29T03:52:11Z_
_Operation: AddBridgeOutputs | Class: validation_

> _Note: this issue was identified against the code base at commit `50acf5c50341569d447ddf04c6c32ab03f586e64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateBridgeState accepts `DesiredState: DELETED`, moves the bridge through DELETING and the clock settles it in the TERMINAL `DELETED` state, where the record is retired but stays in the store and readable (state.rs: "UpdateBridgeState retires the record in this state rather than evicting it"). UpdateBridgeState then refuses to move that bridge anywhere else, "the deletion is irreversible".

AddBridgeOutputs resolves only the bridge ARN: it reads no bridge state at all. A retired (DELETED) bridge therefore takes new network outputs, the call answers 202 as if the change had been accepted, and DescribeBridge afterwards reports the outputs on a record whose deletion the service already reported as final. The crate applies the opposite rule everywhere else it has a terminal retired state: DeleteBridge reads `bridge.bridge_state` before it acts, UpdateGatewayInstance refuses a write to an instance that reached the terminal `DEREGISTERED`, and the router records refuse every later call once a record is on its way out ("an input on its way out is not an input a later call can still act on").

The same missing read of the bridge's state is in the sibling bridge-member paths — AddBridgeSources (add_bridge_sources.rs:211), RemoveBridgeOutput (remove_bridge_output.rs:187) and RemoveBridgeSource (remove_bridge_source.rs:198) all resolve the ARN and mutate without consulting `bridge_state` — so the precondition belongs in the one place every bridge-member mutator reads it from.

## Evidence

AddBridgeOutputs resolves the ARN and appends, with no state precondition — src/ops/add_bridge_outputs.rs:209-213 and :275-281:
```
        // This operation adds outputs to "an existing bridge", addressed by the bridge's
        // own ARN.
        let Some(bridge) = store.bridges.get(&input.bridge_arn) else {
            return Err(not_found(&input.bridge_arn));
        };
...
        let Some(bridge) = store.bridges.get_mut(&input.bridge_arn) else {
            return Err(internal_error(
                "the bridge could not be read back while adding its outputs",
            ));
        };
        let first_added = bridge.outputs.len();
        bridge.outputs.extend(added);
```
The same crate's statement that the state is terminal and the deletion irreversible — src/ops/update_bridge_state.rs:246-264:
```
        // A bridge that has reached, or is reaching, the terminal `DELETED` state cannot be
        // asked for another state: the deletion is irreversible, which is the conflict
        // `ConflictException` describes ...
        if settling == crate::state::BRIDGE_STATE_DELETED
            && settled != crate::state::BRIDGE_STATE_DELETED
        {
```
and src/state.rs:184-193:
```
/// See [`BRIDGE_STATE_CREATING`]. The TERMINAL state of a bridge asked to reach the
/// `DELETED` member of `DesiredState`: ... so UpdateBridgeState retires the record in
/// this state rather than evicting it.
pub const BRIDGE_STATE_DELETED: &str = "DELETED";
```
The sibling bridge operation that DOES read the state before acting — src/ops/delete_bridge.rs:182-185:
```
        // (`crate::state::is_stopped_bridge_state`). Refusing it is a semantic error,
...
        if !crate::state::is_stopped_bridge_state(&bridge.bridge_state) {
```
and the sibling rule for a record in its family's terminal retired state — src/ops/update_gateway_instance.rs:264-270:
```
        // An instance that reached the terminal `DEREGISTERED` is no longer registered with
        // its gateway, so there is no availability to host bridges left to change. The
        // refusal is the semantic error `BadRequestException` is "thrown if the request
        // contains" ...
        if !instance.is_registered() {
```
Test coverage: tests/all/update_bridge_state.rs pins that a DELETED bridge refuses another desired state and stays readable in DELETED; tests/all/bridge_output_admission.rs and tests/all/bridge_add_references.rs exercise AddBridgeOutputs only on live bridges — no test pins that a retired bridge may be grown.

## Steps to Reproduce

1. POST /v1/gateways to create a gateway with one network (e.g. net-1) — 201; keep GatewayArn.
2. POST /v1/bridges with {"name":"b1","placementArn":<GatewayArn>,"egressGatewayBridge":{"maxBitrate":20000000},"sources":[{"flowSource":{"name":"src","flowArn":<a flow's ARN>}}]} — 201; keep BridgeArn. (A cloud-to-ground / egress bridge is the one that takes request-configured network outputs, see state.rs request_bridge_output_rejection.)
3. Wait one transition span so the bridge settles out of CREATING.
4. PUT /v1/bridges/{BridgeArn}/state with {"desiredState":"DELETED"} — 202; wait one transition span.
5. GET /v1/bridges/{BridgeArn} — observe bridgeState == "DELETED" (terminal; UpdateBridgeState now refuses any other desired state with 409).
6. POST /v1/bridges/{BridgeArn}/outputs with {"outputs":[{"networkOutput":{"name":"o1","networkName":"net-1","ipAddress":"203.0.113.1","port":5000,"protocol":"rtp","ttl":64}}]} — observe 202 and the new output reported.
7. GET /v1/bridges/{BridgeArn} — observe the retired DELETED bridge now carrying the output this call added.
