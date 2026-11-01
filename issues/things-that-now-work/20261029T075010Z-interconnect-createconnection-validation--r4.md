Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5f34c6e445f07b3b212c2f20790cdf7255a4bd24

# An attachPoint.arn is accepted while the connection behind it has no provisioned connectivity, unlike the gateway member beside it

_Filed: 2026-10-29T07:50:10Z_
_Operation: CreateConnection | Class: validation_

> _Note: this issue was identified against the code base at commit `5f34c6e445f07b3b212c2f20790cdf7255a4bd24`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two members of `AttachPoint` are judged by two different readiness rules, although the crate says they share one.

For `AttachPoint$directConnectGateway` the crate requires the gateway to be ready for use: `descriptor_of` drops every gateway whose state is not `available`, so a gateway that is still being created (`pending`) is refused as "not an attach point valid for environment …".

For `AttachPoint$arn` the only state filter is `!is_deleted_or_deleting`, so a connection in `requested` — the state the model documents as "The initial state of a connection. The state will remain here until the Connection is accepted on the Partner portal" — or in `pending` ("being provisioned between AWS and the Partner") resolves as a valid attach point and a second Connection is provisioned onto connectivity that does not exist yet. The same holds for `failed`/`down`. The helper's own doc comment claims the opposite: it says the ARN filter is "the same readiness rule the union's gateway member is held to", naming only the torn-down half of that rule.

The window is deterministic and reachable with two back-to-back calls: a connection stays in `requested` for `ACTIVATION_SPAN` (250 ms) and then in `pending` for another 250 ms, exactly the "very next call" pattern this crate's own test suite uses. The effect is a stored `Connection` whose `attachPoint` names a circuit that was never confirmed at any partner portal — and if the referenced connection never gets accepted, the attachment is onto connectivity that will never exist.

This is NOT the gap the fixed issue "An attachPoint.arn naming a connection whose delete has already been called still resolves as a valid attach point" closed (that added the `is_deleted_or_deleting` filter, which must stay); it is the not-yet-provisioned half of the same readiness rule.

## Evidence

The gateway member IS held to a readiness rule — services/service-com-amazonaws-interconnect-interconnect/src/state/direct_connect.rs:24-30:
```
/// The one `DirectConnectGatewayState` a Connection can be attached to.
///
/// Direct Connect documents the value as "available: The Direct Connect gateway is ready
/// for use"; a gateway that is still being created (`pending`) or being torn down
/// (`deleting`) is not ready for use, so it is not yet — or no longer — a valid attach
/// point.
const GATEWAY_READY_STATE: &str = "available";
```
and enforced at direct_connect.rs:84-89:
```
    if gateway
        .direct_connect_gateway_state()
        .is_none_or(|state| state.as_str() != GATEWAY_READY_STATE)
    {
        return None;
    }
```

The ARN member is NOT, while claiming it is — src/state.rs:499-513:
```
    /// connection behind it the handle names nothing ([`is_deleted_or_deleting`]) —
    /// the same filter [`Self::proposal_in`] applies to the other handle of that kind,
    /// and the same readiness rule the union's gateway member is held to, where a
    /// gateway being torn down is no longer a valid attach point
    /// ([`direct_connect`]). …
    pub fn attach_point_connection_in(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&StoredConnection> {
        self.connection_in_by_arn(region, arn)
            .filter(|stored| !is_deleted_or_deleting(&stored.connection.state))
    }
```
reached for every create/acceptance through src/state.rs:808-812:
```
    if environment_offers_attach_points(store.environments.get(environment_id))
        && store.attach_point_connection_in(region, arn).is_some()
    {
        return Ok(());
    }
```
(called from src/ops/create_connection.rs:322 and src/ops/accept_connection_proposal.rs:292).

The model says what state `requested` means — smithy_query(query="shape", shape="Connection"), member `state`:
```
<code>requested</code>: The initial state of a connection. The state will remain here until the Connection is accepted on the Partner portal.
<code>pending</code>: The connection has been accepted and is being provisioned between AWS and the Partner.
<code>available</code>: The connection has been fully provisioned between AWS and the Partner.
```
and smithy_query(query="doc", shape="com.amazonaws.interconnect#CreateConnection"): "the resulting Connection will return an Activation Key which will need to be brought to the specific partner's portal to confirm the Connection on both sides." So a `requested`/`pending` connection is the exact analogue of the `pending` gateway the crate already refuses.

A new connection is armed in `requested` on every create — src/ops/create_connection.rs:475 and :518:
```
            state: "requested".to_string(),
…
                pending_transition: crate::state::owed_progression(&connection.state, now),
```

Test coverage: tests/all/attach_point_arn_stops_at_the_delete.rs:6-14 states the rule as "an attach point is a handle whose whole subject is the connectivity a Connection is provisioned onto … The readiness rule is the union's, not one member's: the gateway member is refused once its gateway is no longer ready for use, and the ARN member is refused once its connection's teardown has been called." Its rows only offer an ARN after a `DeleteConnection` (`Offered::WhileTearingDown` / `OnceTornDown`, lines 40-49); no test in the suite offers the ARN of a connection in `requested` or `pending`, so the not-yet-provisioned case is unpinned.

## Steps to Reproduce

1. Boot the emulator and create one ready Direct Connect gateway G (state available), as tests/all/attach_points.rs does.
2. Call ListEnvironments, pick a creatable environmentId and one of its available bandwidths.
3. Call CreateConnection with attachPoint={"directConnectGateway": G}; note connection A's arn and that A.state == "requested".
4. As the very next call (inside the 250 ms ACTIVATION_SPAN), call CreateConnection again on the same environment with attachPoint={"arn": <A.arn>}.
5. Observed: HTTP 200 — connection B is stored attached to A, a connection that has not been confirmed at any partner portal and whose connectivity does not exist ("the state will remain here until the Connection is accepted on the Partner portal"). GetConnection on B reports attachPoint.arn = A.arn.
6. Contrast: create a Direct Connect gateway that is not in state `available` and offer it as attachPoint.directConnectGateway — the create is refused 404 with attach_point_not_valid_message, because descriptor_of drops a gateway that is not ready for use. The same request shape is accepted for the ARN member and refused for the gateway member.
