Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a19f71fb9b9d334f8efc64d442ea8394bd20139d

# DeleteGateway counts bridges retired in the terminal DELETED state as still placed on the gateway, while the instance half of the same precondition excludes terminally deregistered instances

_Filed: 2026-10-28T15:07:30Z_
_Operation: DeleteGateway | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a19f71fb9b9d334f8efc64d442ea8394bd20139d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteGateway enforces the documented precondition "Before you can delete a gateway, you must deregister its instances and delete its bridges" in two halves. The instance half goes through `registered_gateway_instance_count`, which the crate documents as excluding a record that has reached its TERMINAL retired state. The bridge half is hand-counted in the handler as every bridge whose `placement_arn` matches, with no state filter — so a bridge that UpdateBridgeState already drove to the terminal `DELETED` state (which this crate's own constant doc calls a RETIRED record kept only so it stays readable) still blocks the gateway delete with a 400 telling the caller to "Delete the bridges of a gateway", for bridges that are already deleted.

The two halves of one sentence therefore read "its bridges"/"its instances" differently: a terminally retired instance is no longer the gateway's, a terminally retired bridge still is.

## Evidence

The instance half excludes terminal records — services/service-com-amazonaws-mediaconnect-mediaconnect/src/state.rs:5941-5948:
```
    /// A deregistered instance stays readable in its terminal `DEREGISTERED` state but is
    /// no longer one of the gateway's instances, exactly as a retired gateway is no longer
    /// one of the account's ([`Self::live_gateway_count`]).
    pub fn registered_gateway_instance_count(&self, gateway_arn: &str) -> usize {
        self.gateway_instances
            .values()
            .filter(|instance| instance.gateway_arn == gateway_arn && instance.is_registered())
            .count()
    }
```
The bridge half counts every placed record — src/ops/delete_gateway.rs:209-225:
```
            let placed = store
                .bridges
                .values()
                .filter(|bridge| bridge.placement_arn == gateway_arn)
                .count();
            if placed > 0 {
                return Err(DeleteGatewayErrorModeled::BadRequestException(
                    BadRequestException {
                        message: crate::state::bad_request_message(&format!(
                            "Gateway {gateway_arn} still has {placed} bridge(s) placed on \
                             it. Delete the bridges of a gateway before deleting the \
                             gateway."
```
and the crate states that a `DELETED` bridge is a retired record — src/state.rs:184-192:
```
/// See [`BRIDGE_STATE_CREATING`]. The TERMINAL state of a bridge asked to reach the
/// `DELETED` member of `DesiredState`: its presence in the `BridgeState` enum is the
/// model's own statement that such a bridge stays a readable resource, exactly as
/// [`GATEWAY_STATE_DELETED`] is for a gateway, so UpdateBridgeState retires the record in
/// this state rather than evicting it.
```
The same crate also already filters bridges by state where it counts them for the OTHER precondition — src/state.rs:5913-5919 (`running_bridge_count`: `&& !is_stopped_bridge_state(&bridge.bridge_state)`).

Test coverage: tests/all/bridge_members_and_deletion.rs:1016-1073 (`a_gateway_a_bridge_is_placed_on_is_deleted_only_after_the_bridge`) uses a LIVE bridge and then DeleteBridge; no test places a bridge retired via UpdateBridgeState `DesiredState: DELETED` on the gateway, so nothing pins the current count.

## Steps to Reproduce

1. POST /v1/gateways to create a gateway with one network.
2. POST /v1/bridges placing one bridge on that gateway.
3. PUT /v1/bridges/{BridgeArn}/state with `{"desiredState":"DELETED"}` and wait one transition span; GET /v1/bridges/{BridgeArn} reports `bridgeState: "DELETED"`.
4. DELETE /v1/gateways/{GatewayArn} — observe 400 BadRequestException "Gateway ... still has 1 bridge(s) placed on it. Delete the bridges of a gateway before deleting the gateway." although the bridge is in the terminal DELETED state.
5. Compare with the instance half: register an instance, DELETE /v1/gateway-instances/{Arn} until it reports the terminal DEREGISTERED, and observe DeleteGateway no longer counts it.
