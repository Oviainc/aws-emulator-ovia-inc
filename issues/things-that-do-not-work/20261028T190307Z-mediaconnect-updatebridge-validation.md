Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7cfd583752b909e6f589468ac42594db7887f86f

# UpdateBridge reconfigures a bridge retired in the terminal DELETED state, which the crate's own UpdateBridgeState calls irreversible

_Filed: 2026-10-28T19:03:07Z_
_Operation: UpdateBridge | Class: validation_

> _Note: this issue was identified against the code base at commit `7cfd583752b909e6f589468ac42594db7887f86f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateBridgeState accepts `DesiredState: DELETED`, moves the bridge through DELETING and the clock settles it in the TERMINAL `DELETED` state, where the record is retired but stays in the store and readable. UpdateBridgeState itself then refuses to move that bridge anywhere else, because "the deletion is irreversible".

UpdateBridge resolves only the bridge ARN: it reads no bridge state at all, so a retired (DELETED) bridge is reconfigured — max bitrate, max outputs and the whole source failover config are written onto a record whose deletion the service already reported as final — and the call answers 202 as if the change had been accepted. The crate applies the opposite rule everywhere else it has a terminal retired state: UpdateGatewayInstance refuses a write to an instance that reached the terminal `DEREGISTERED`, and the router records refuse every later call once a record is on its way out ("an input on its way out is not an input a later call can still act on").

## Evidence

UpdateBridge resolves the ARN and writes, with no state precondition — src/ops/update_bridge.rs:242-244 and :327-329:
```
        let Some(bridge) = store.bridges.get_mut(&input.bridge_arn) else {
            return Err(not_found(&input.bridge_arn));
        };
...
        bridge.egress_gateway_bridge = egress_gateway_bridge;
        bridge.ingress_gateway_bridge = ingress_gateway_bridge;
        bridge.source_failover_config = source_failover_config;
```
The same crate's statement that the state is terminal and the deletion irreversible — src/ops/update_bridge_state.rs:246-264:
```
        // A bridge that has reached, or is reaching, the terminal `DELETED` state cannot be
        // asked for another state: the deletion is irreversible, which is the conflict
        // `ConflictException` describes ...
        if settling == crate::state::BRIDGE_STATE_DELETED
            && settled != crate::state::BRIDGE_STATE_DELETED
```
and src/state.rs:184-193:
```
/// See [`BRIDGE_STATE_CREATING`]. The TERMINAL state of a bridge asked to reach the
/// `DELETED` member of `DesiredState`: ... so UpdateBridgeState retires the record in
/// this state rather than evicting it.
pub const BRIDGE_STATE_DELETED: &str = "DELETED";
```
The sibling rule for a record in its family's terminal retired state — src/ops/update_gateway_instance.rs:264-280:
```
        // An instance that reached the terminal `DEREGISTERED` is no longer registered with
        // its gateway, so there is no availability to host bridges left to change. The
        // refusal is the semantic error `BadRequestException` is "thrown if the request
        // contains" ...
        if !instance.is_registered() {
```
and src/state.rs:3968-3974:
```
    /// Whether a state change is in flight for this input: ... including the deletion
    /// it owes once DeleteRouterInput has accepted one, because an input on its way out is
    /// not an input a later call can still act on.
    pub fn is_transitioning(&self) -> bool {
```
Test coverage: tests/all/update_bridge_state.rs:364-416 pins that a DELETED bridge refuses another desired state and stays readable in DELETED; tests/all/update_bridge.rs exercises UpdateBridge only on live bridges — no test pins that a retired bridge may be reconfigured.

## Steps to Reproduce

1. POST /v1/gateways to create a gateway, then POST /v1/bridges to create a bridge on it — 201; keep BridgeArn.
2. Wait one transition span so the bridge settles in STANDBY.
3. PUT /v1/bridges/{BridgeArn}/state with {"desiredState":"DELETED"} — 202; wait one transition span.
4. GET /v1/bridges/{BridgeArn} — observe bridgeState == "DELETED" (terminal, and UpdateBridgeState now refuses any other desired state with 409).
5. PUT /v1/bridges/{BridgeArn} with {"egressGatewayBridge":{"maxBitrate":80000000}} (or a sourceFailoverConfig) — observe 202 and GET /v1/bridges/{BridgeArn} reporting the new configuration on the retired bridge.
