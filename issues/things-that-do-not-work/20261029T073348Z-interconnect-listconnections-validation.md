Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2b09bad8439b0b0ac599f9d638bea059710099b4

# The `provider` list filter's union member is never held to its modelled length, unlike the `attachPoint` filter beside it

_Filed: 2026-10-29T07:33:48Z_
_Operation: ListConnections | Class: validation_

> _Note: this issue was identified against the code base at commit `2b09bad8439b0b0ac599f9d638bea059710099b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListConnections` carries two union-typed filters, `attachPoint` (AttachPoint) and `provider` (Provider). The handler holds the `attachPoint` union's member to its own modelled `@length`/`@pattern` before filtering, and states why: nothing before the handler validates a member one level down inside a union, so a value the member cannot hold is a fault in the request rather than something to compare against every stored connection and answer with an empty page. The sibling `provider` filter is given no such check: its value goes straight into `provider_eq` as a plain string comparison. `Provider`'s two members both target shapes carrying `@length(max: 32)`, so a filter such as `{"cloudServiceProvider": "<40 characters>"}` — a value the model forbids — is accepted and answered HTTP 200 with an empty `connections` page, which the caller cannot tell from "nothing matched". The same unvalidated `provider` filter is the one `ListEnvironments` uses (list_environments.rs:161-172), so the gap is in both readers of that union, while the crate's third union (`RemoteAccountIdentifier`) IS hand-checked by `remote_account_violation` for exactly the stated reason. Two of this crate's three unions are therefore held to their members' modelled constraints and one is not.

## Evidence

The rule the crate states for itself, and applies to the sibling filter in the SAME handler — services/service-com-amazonaws-interconnect-interconnect/src/ops/list_connections.rs:155-171:
```
        // The `attachPoint` filter carries the same union a create carries, and its
        // members are constrained in the model while nothing before the handler holds a
        // member one level down to those constraints: a value that is not an
        // attach-point identifier of its kind is a fault in the REQUEST, refused as
        // invalid input by the one rule every reader of an `AttachPoint` applies
        // (`crate::state::attach_point_violation`), rather than compared against every
        // stored connection and answered with the empty page it can never match.
        if let Some(message) = input
            .attach_point
            .as_ref()
            .and_then(crate::state::attach_point_violation)
        {
            return Err(ListConnectionsErrorModeled::InterconnectValidationException(
                InterconnectValidationException { message },
            )
            .into());
        }
```
The `provider` filter, in the same function, with no such check — list_connections.rs:199-201:
```
                        && input.provider.as_ref().is_none_or(|wanted| {
                            crate::state::provider_eq(&connection.provider, wanted)
                        })
```
and the comparison it calls, which judges nothing about the value — src/state.rs:577-583:
```
pub fn provider_eq(left: &Provider, right: &Provider) -> bool {
    match (left, right) {
        (Provider::cloudServiceProvider(a), Provider::cloudServiceProvider(b))
        | (Provider::lastMileProvider(a), Provider::lastMileProvider(b)) => a == b,
        _ => false,
    }
}
```
The crate-wide rule, stated once for the attach-point union and naming the remote-account union as the other place it is applied — src/state.rs:742-750:
```
/// The union's members are constrained in the model and NOTHING before the handler holds
/// them to it: the boundary validator holds a top-level member to its target's
/// constraints but not a member one level down, inside a union — which is why the sibling
/// union is hand-checked the same way ([`remote_account_violation`]). So every reader of
/// an `AttachPoint` judges the value here first, and a value outside the member's
/// constraints is a fault in the REQUEST, reported as invalid input, rather than an
/// identifier resolved against state and then reported as a resource the account does not
/// hold
```
and repeated for the third union — src/state.rs:826-828:
```
/// Checked here because the boundary validator holds a top-level member to its
/// target's constraints but not a member one level down, inside a union.
```
The constraint the `provider` members carry — smithy_query(query="shape", shape="Provider"):
```
"cloudServiceProvider": {"target": "CloudServiceProvider", "length": {"max": 32}, ...}
"lastMileProvider": {"target": "LastMileProvider", "length": {"max": 32}, ...}
```
Test coverage: the attachPoint half of the rule is pinned end to end for this very filter — tests/all/attach_point_member_constraints.rs:521-553 ("RULE: the `ListConnections` `attachPoint` filter carries the same union, so it holds the same member constraints — a value the member cannot hold is the same 400 ... rather than a page the caller cannot tell from 'nothing matched'", asserting status 400 and `InterconnectValidationException`). No test sends an out-of-constraint `provider`: the only provider filters exercised are catalog values and the in-constraint miss `{"cloudServiceProvider": "NoSuchPartner"}` (tests/all/connection_lifecycle.rs:200-211), so the gap is unpinned.

## Steps to Reproduce

1. Boot the emulator and call ListEnvironments to confirm the seeded catalog is served.
2. POST / with X-Amz-Target: Interconnect.ListConnections and body {"attachPoint": {"directConnectGateway": "not-a-gateway-id"}} — observe HTTP 400 InterconnectValidationException, the handler holding the union member to its modelled pattern.
3. POST / with X-Amz-Target: Interconnect.ListConnections and body {"provider": {"cloudServiceProvider": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}} (40 characters, past the member's @length(max: 32)).
4. Observed: HTTP 200 with an empty `connections` list — the same answer a well-formed provider that matches nothing gets. Expected, by the rule the handler applies to its sibling filter: the modelled InterconnectValidationException (400) naming the value the member cannot hold.
5. Repeat step 3 against Interconnect.ListEnvironments: HTTP 200 with an empty `environments` list for the same out-of-constraint value.
