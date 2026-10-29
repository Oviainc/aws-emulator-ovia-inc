Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1211f7611505016a47ab1bf8932fdc778b7fd399

# UpdateBridgeOutput reconfigures an output of a bridge retired in the terminal DELETED state, which the crate's own UpdateBridgeState calls irreversible

_Filed: 2026-10-29T01:46:32Z_
_Operation: UpdateBridgeOutput | Class: validation_

> _Note: this issue was identified against the code base at commit `1211f7611505016a47ab1bf8932fdc778b7fd399`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateBridgeState accepts `DesiredState: DELETED`, puts the bridge through DELETING and the clock settles it in the TERMINAL `DELETED` state; the record is retired but stays in the store and readable. UpdateBridgeState then refuses to move that bridge anywhere else because, in its own words, "the deletion is irreversible".

UpdateBridgeOutput reads no bridge state at all: it resolves the bridge ARN, resolves the output position, validates the requested `NetworkName` against the placement gateway, and writes the merged `BridgeOutput` into the retired record — answering 202 as if the reconfiguration had been accepted. DescribeBridge afterwards reports the retired, DELETED bridge carrying the new output settings.

The crate applies the opposite rule everywhere else it has a terminal retired state: UpdateGatewayInstance refuses a write to an instance that reached the terminal `DEREGISTERED`, and every router operation refuses a record whose removal is owed because "an input on its way out is not an input a later call can still act on".

## Evidence

UpdateBridgeOutput resolves the ARN and writes, with no state precondition anywhere in the handler — src/ops/update_bridge_output.rs:252-261 and :327-333:
```
        let Some(bridge) = store.bridges.get(&input.bridge_arn) else {
            return Err(not_found(&input.bridge_arn));
        };
...
        let Some(position) = bridge.output_position(&input.output_name) else {
            return Err(not_found(&input.output_name));
        };
...
            let Some(bridge) = store.bridges.get_mut(&input.bridge_arn) else {
                return Err(not_found(&input.bridge_arn));
            };
            let Some(slot) = bridge.outputs.get_mut(position) else {
                return Err(not_found(&input.output_name));
            };
            *slot = merged;
```
The same crate's statement that the state is terminal and the deletion irreversible — src/ops/update_bridge_state.rs:246-264:
```
        // A bridge that has reached, or is reaching, the terminal `DELETED` state cannot be
        // asked for another state: the deletion is irreversible, which is the conflict
        // `ConflictException` describes ("would cause a conflict with the current state of a
        // service resource associated with the request", 409 and retryable). ...
        if settling == crate::state::BRIDGE_STATE_DELETED
            && settled != crate::state::BRIDGE_STATE_DELETED
```
and src/state.rs:184-193:
```
/// See [`BRIDGE_STATE_CREATING`]. The TERMINAL state of a bridge asked to reach the
/// `DELETED` member of `DesiredState`: its presence in the `BridgeState` enum is the
/// model's own statement that such a bridge stays a readable resource, ... so
/// UpdateBridgeState retires the record in this state rather than evicting it.
pub const BRIDGE_STATE_DELETED: &str = "DELETED";
```
The sibling rule for a record sitting in its family's terminal retired state — src/ops/update_gateway_instance.rs:264-280:
```
        // An instance that reached the terminal `DEREGISTERED` is no longer registered with
        // its gateway, so there is no availability to host bridges left to change. ...
        if !instance.is_registered() {
```
Test coverage: tests/all/update_bridge_state.rs pins that a DELETED bridge refuses another desired state and stays readable; a grep of tests/all/update_bridge_output.rs for `DELETED|desiredState` returns no matches, so no test pins that a retired bridge's outputs may be reconfigured.

## Steps to Reproduce

1. POST /v1/gateways to create a gateway — 201; wait one transition span.
2. POST /v1/bridges on that gateway with one network output — 201; keep BridgeArn and the output name; wait one transition span so the bridge settles in STANDBY.
3. PUT /v1/bridges/{BridgeArn}/state with {"desiredState":"DELETED"} — 202; wait one transition span.
4. GET /v1/bridges/{BridgeArn} — observe bridgeState == "DELETED" (UpdateBridgeState now answers 409 for any other desired state).
5. PUT /v1/bridges/{BridgeArn}/outputs/{OutputName} with {"networkOutput":{"port":5001,"ttl":32}} — observe 202.
6. GET /v1/bridges/{BridgeArn} — observe the retired DELETED bridge reporting the new port and ttl.
