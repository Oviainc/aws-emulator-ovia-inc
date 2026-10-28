Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b45fe0b1030ed2b932d59ccf686ccc68d031b528

# An attachPoint.arn naming a connection whose delete has already been called still resolves as a valid attach point

_Filed: 2026-10-28T15:16:52Z_
_Operation: CreateConnection | Class: validation_

> _Note: this issue was identified against the code base at commit `b45fe0b1030ed2b932d59ccf686ccc68d031b528`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateConnection` (and the identical branch in `AcceptConnectionProposal`) resolves an `AttachPoint::Arn` through `store.connection_in_by_arn`, whose only state filter is `holds_arn`, and `holds_arn` excludes just the TERMINAL `deleted` state. A connection on which `DeleteConnection` has already been called — i.e. one in the transient `deleting` state, for the whole 400 ms `PROVISIONING_SPAN` the teardown takes — therefore still resolves, and a new Connection is provisioned onto it with HTTP 200.

That contradicts two rules this same crate states:

1. The crate's own helper `is_deleted_or_deleting` exists precisely for "a handle whose whole subject is that connectivity" and documents that such a handle "names nothing from the moment the delete is called". `proposal_in` calls it (so an activation key stops resolving the instant the delete is called), but the attach-point resolution path never calls it, even though an attach point is exactly a handle whose whole subject is the connectivity the new Connection is provisioned onto.
2. The sibling branch of the SAME union in the SAME `judge_against_store`/`handle` pair — `AttachPoint::directConnectGateway` — refuses an attach point that is being torn down: `direct_connect::descriptor_of` keeps only gateways in `GATEWAY_READY_STATE`, documented as "a gateway that is still being created (`pending`) or being torn down (`deleting`) is not ready for use, so it is not yet — or no longer — a valid attach point". So the readiness rule is applied to one member of `AttachPoint` and skipped on the other, and which rule a caller gets depends only on which member it sent.

The net effect is a 400 ms window in which the service accepts a brand-new connection attached to connectivity it has already agreed to tear down, and after which the exact same request is refused 404 — the answer depends on how fast the caller is rather than on the request.

## Evidence

The rule the crate states for a handle whose subject is the connectivity — services/service-com-amazonaws-interconnect-interconnect/src/state.rs:1024-1033:
```
/// Whether a `DeleteConnection` has been made on a connection in `state`: it is
/// either being torn down or has been.
///
/// Either way "the network connectivity provided by the Connection will cease to
/// exist", so a handle whose whole subject is that connectivity names nothing from
/// the moment the delete is called — while the record itself stays readable by its
/// identifier, which is how a caller polling the delete sees the teardown.
pub fn is_deleted_or_deleting(state: &str) -> bool {
    state == DELETING_CONNECTION_STATE || state == DELETED_CONNECTION_STATE
}
```
The path that honours it — state.rs:399-404 (`proposal_in`):
```
            .and_then(|connection_id| self.connection_in(region, connection_id))
            .filter(|stored| !is_deleted_or_deleting(&stored.connection.state))
```
The attach-point path that does NOT — state.rs:1035-1039:
```
/// Whether `stored` is the live resource `arn` names: the stored ARN matches field
/// for field, and the connection has not reached the terminal deleted state.
fn holds_arn(stored: &StoredConnection, arn: &str) -> bool {
    stored.connection.arn == arn && stored.connection.state != DELETED_CONNECTION_STATE
}
```
reached from state.rs:442-447 (`connection_in_by_arn`) by src/ops/create_connection.rs:289-301:
```
    if let AttachPoint::Arn(arn) = &input.attach_point {
        if store.connection_in_by_arn(region, arn).is_none() {
            return Err(CreateConnectionErrorModeled::ResourceNotFoundException(
```
(the identical branch is src/ops/accept_connection_proposal.rs:257-271).

The sibling union member that DOES apply a readiness rule — src/state/direct_connect.rs:20-26:
```
/// The one `DirectConnectGatewayState` a Connection can be attached to.
///
/// Direct Connect documents the value as "available: The Direct Connect gateway is ready
/// for use"; a gateway that is still being created (`pending`) or being torn down
/// (`deleting`) is not ready for use, so it is not yet — or no longer — a valid attach
/// point.
const GATEWAY_READY_STATE: &str = "available";
```
enforced at direct_connect.rs:80-85, and reached from create_connection.rs:361-386.

The window exists because `DeleteConnection` only ARMS the terminal state — src/ops/delete_connection.rs:170-176:
```
        if !crate::state::is_deleted_or_deleting(&stored.connection.state) {
            stored.connection.state = crate::state::DELETING_CONNECTION_STATE.to_string();
            stored.pending_transition = Some(crate::state::PendingTransition {
                due: now + crate::state::PROVISIONING_SPAN,
```
with `PROVISIONING_SPAN` = 400 ms (state.rs:60).

Test coverage: the `arn` attach-point suite covers only the SETTLED case, never the teardown window — tests/all/create_connection_attach_point_validation.rs:353-364:
```
        // The SAME ARN, once the connection it names is gone, resolves to nothing.
        let base_id = text(&base["connection"], "id");
        emulator
            .ic_ok_in(region, "DeleteConnection", json!({ "identifier": base_id }))
            .await;
        tokio::time::sleep(SETTLE_WAIT).await;
```
with `SETTLE_WAIT` = 700 ms (same file:27-29, "Long enough to cover a connection's tear-down span (the handler's own 400 ms) with margin"). No row calls the create inside that span, so the behaviour described here is unpinned.

Distinct from the already-fixed CreateConnection/validation issue "accepts an attachPoint.arn from another Region" (20261028T013820Z-interconnect-createconnection-validation--r2): that one added the `region` argument to this very call, which is present in the code quoted above; this one is about the STATE of the connection the ARN resolves to, which no issue has covered.

## Steps to Reproduce

1. Mount Interconnect beside Direct Connect (as tests/all/emulator.rs does) and create a ready Direct Connect gateway.
2. CreateConnection in us-east-1 on a seeded environment (e.g. env-csp-oraclecloud-eu-west-1 or env-csp-azure-us-east-1 with the remoteAccount it asks for), attachPoint = {"directConnectGateway": "<gateway>"}; note connection A's `arn`.
3. Call DeleteConnection with A's identifier; observe HTTP 200 with `connection.state == "deleting"` — the teardown is armed for 400 ms.
4. Within that 400 ms, call CreateConnection again with attachPoint = {"arn": "<A.arn>"}.
5. Observe HTTP 200: a new connection is provisioned onto an attach point whose delete has already been called, although src/state.rs:1024-1030 says such a handle "names nothing from the moment the delete is called" and src/state/direct_connect.rs:20-26 refuses the gateway form of an attach point that is "being torn down".
6. Confirm the window is the only difference: in the same window, DescribeConnectionProposal with A's activationKey answers 404 (it filters through `is_deleted_or_deleting`); wait 700 ms and repeat step 4 — now it is refused 404.
