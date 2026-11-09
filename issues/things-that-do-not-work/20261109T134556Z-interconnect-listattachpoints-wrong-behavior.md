Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 01e7e7e4d8371571a5c4b5afb30bf8b87d00507b

# ListAttachPoints offers only gateways, while the two attach-point writers also accept connection ARNs the listing never reports

_Filed: 2026-11-09T13:45:56Z_
_Operation: ListAttachPoints | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `01e7e7e4d8371571a5c4b5afb30bf8b87d00507b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state.rs`'s own module documentation states the rule this crate claims to hold: an attach point is a Direct Connect gateway, `ListAttachPoints` resolves attach points against the service that owns them "rather than reading a catalog of this service's own", and `CreateConnection` "resolves the attach point it is given through that same walk, so what a Connection may be attached to is what the listing offers."

The code breaks both halves of that sentence for the `AttachPoint$arn` member:

* `CreateConnection` and `AcceptConnectionProposal` resolve an `arn` attach point through `crate::state::judge_attach_point_in_store`, which reads exactly the catalog the doc says is not read — this service's own `connections` map, via `ComAmazonawsInterconnectInterconnectState::attach_point_connection_in`. The Direct Connect walk (`direct_connect::judge_attach_point`) returns `Ok(())` for that member without resolving anything.
* `ListAttachPoints` enumerates only `direct_connect::attach_points`, i.e. the account's Direct Connect gateways. No connection ARN is ever in the page, so the set of attach points a create accepts is strictly larger than the set the listing offers.

The consequence is observable: a client that uses `ListAttachPoints` to discover "all Attach Points the caller has access to that are valid for the specified Environment" (the operation's modelled purpose) can create a connection onto an `attachPoint.arn` that the same endpoint, in the same Region, for the same Environment, never listed — the opposite of the invariant the crate states for itself. One of the two sides is wrong: either the listing must report the connection-ARN attach points its sibling handlers accept, or the stated rule and the ARN branch of the create path do not belong together.

## Evidence

The rule the crate states for itself — services/service-com-amazonaws-interconnect-interconnect/src/state.rs:20-24:
```
//! Attach points are NOT held here: an attach point is a Direct Connect gateway of the
//! account, so `ListAttachPoints` resolves them against the service that owns them
//! ([`direct_connect`]) rather than reading a catalog of this service's own — and
//! `CreateConnection` resolves the attach point it is given through that same walk, so
//! what a Connection may be attached to is what the listing offers.
```
The create path resolving an ARN attach point against this service's OWN store, not that walk — src/state.rs:899-918:
```
pub fn judge_attach_point_in_store<E: direct_connect::AttachPointRefusal>(
    store: &ComAmazonawsInterconnectInterconnectState,
    region: &str,
    attach_point: &AttachPoint,
    environment_id: &str,
) -> Result<(), E> {
    judge_attach_point_value::<E>(attach_point)?;
    let AttachPoint::Arn(arn) = attach_point else {
        return Ok(());
    };
    if environment_offers_attach_points(environment_in_region(region, environment_id).as_ref())
        && store.attach_point_connection_in(region, arn).is_some()
    {
        return Ok(());
    }
```
and the store lookup it reaches — src/state.rs:535-542:
```
    pub fn attach_point_connection_in(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&StoredConnection> {
        self.connection_in_by_arn(region, arn)
            .filter(|stored| provides_connectivity(&stored.connection.state))
    }
```
reached from the create — src/ops/create_connection.rs:367-372:
```
    crate::state::judge_attach_point_in_store::<CreateConnectionError>(
        store,
        region,
        &input.attach_point,
        &environment.environment_id,
    )?;
```
and from the acceptance — src/ops/accept_connection_proposal.rs:373-378 (`judge_attach_point_in_store::<AcceptConnectionProposalError>(store, region, &input.attach_point, &connection.environment_id)?`).

The listing, which reads only the gateway side and so can never contain those ARNs — src/ops/list_attach_points.rs:215-222:
```
        let descriptors = match crate::state::direct_connect::attach_points(
            ctx,
            &region,
            Some(&environment),
        )
        .await
        {
            crate::state::direct_connect::AttachPointListing::Resolved(descriptors) => descriptors,
```
and what that reader is — src/state/direct_connect.rs:271-274:
```
/// The attach points valid for `environment`: every Direct Connect gateway of the
/// account that is ready for use, as the descriptors `ListAttachPoints` reports — and
/// NONE at all for an Environment that carries no Connection
```
Test coverage: tests/all/attach_point_arn_needs_provisioned_connectivity.rs:6-14 pins that an `AttachPoint$arn` IS a valid attach point once the connection behind it is `available` ("an attach point is a handle whose whole subject is the connectivity a Connection is provisioned onto"), and tests/all/attach_points.rs / tests/all/caller_created_gateways_are_attach_points.rs pin the listing as the account's Direct Connect gateways only; no test asserts that the listing is the complete set of attach points a create accepts, which is why the stated invariant survives unchecked.

## Steps to Reproduce

1. Boot the emulator and point an Interconnect client at it.
2. Call ListEnvironments to get the called Region's environmentId and its available bandwidths.
3. Create a first connection onto a Direct Connect gateway attach point and poll GetConnection until its state is `available`. Note its `arn`.
4. Call ListAttachPoints(environmentId=<that id>) and observe the page: every descriptor has type `DirectConnectGateway` and the identifier of a Direct Connect gateway; the `arn` of the connection from step 3 is not in it.
5. Call CreateConnection with attachPoint={"arn": "<the arn from step 3>"} on the same environmentId. It succeeds — the connection is attached to something ListAttachPoints never offered.
6. Compare with src/state.rs:20-24, which states that "what a Connection may be attached to is what the listing offers".
