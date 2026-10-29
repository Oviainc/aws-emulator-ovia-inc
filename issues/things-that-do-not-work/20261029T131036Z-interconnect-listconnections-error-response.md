Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7bcaea858dd8788316443d36e8dbe675c4b82465

# The environmentId filter answers an unknown Environment with an empty page, the one answer this handler's own comment and its sibling say must never be given

_Filed: 2026-10-29T13:10:36Z_
_Operation: ListConnections | Class: error-response_

> _Note: this issue was identified against the code base at commit `7bcaea858dd8788316443d36e8dbe675c4b82465`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListConnections` holds two of its four filters to a rule it states in its own body: a filter value that can never match any stored record is a fault in the REQUEST and is refused, "rather than compared against every stored connection and answered with the empty page it can never match". The `environmentId` filter beside them is exempted: it is compared verbatim against `connection.environment_id` with no lookup in the Environment catalog at all, so an `environmentId` the catalog does not hold — a value no stored connection can ever carry, because `CreateConnection` refuses to allocate a connection on an environment the catalog does not hold — is answered HTTP 200 with an empty `connections` list.

That is exactly the answer this crate's sibling environment reader says must never be produced. `ListAttachPoints` refuses an absent `environmentId` with `InterconnectClientException` and justifies it as "the 400 `InterconnectClientException` every environment-resolving operation gives, never the connection-scoped 404 and never an empty page, which a caller could not tell from 'this environment offers none'"; `CreateConnection` and `GetEnvironment` give the same refusal through the same shared message helper (`crate::state::environment_not_found_message`). `ListConnections` resolves an `environmentId` too and is the one operation of the four that gives neither the refusal nor any way for a caller to tell "that environment does not exist" from "that environment carries no connection of yours".

The helper and the refusal wording already exist in the crate (`crate::state::environment_not_found_message`, used by three handlers) and this handler already holds the lock on the store that carries the catalog (`store.environments`), so nothing new has to be invented: the lookup the three siblings perform is simply not performed here.

## Evidence

The rule, stated inside this very handler for the two filters that DO get checked — services/service-com-amazonaws-interconnect-interconnect/src/ops/list_connections.rs:155-171:
```
        // The `attachPoint` filter carries the same union a create carries, and its
        // members are constrained in the model while nothing before the handler holds a
        // member one level down to those constraints: a value that is not an
        // attach-point identifier of its kind is a fault in the REQUEST, refused as
        // invalid input by the one rule every reader of an `AttachPoint` applies
        // (`crate::state::attach_point_violation`), rather than compared against every
        // stored connection and answered with the empty page it can never match.
```
and list_connections.rs:173-177:
```
        // The `provider` filter is the other union on this input, and the same rule
        // decides it: its members are constrained in the model too, so a value the
        // member cannot hold is refused by the one rule every reader of a `Provider`
        // applies (`crate::state::provider_violation`) rather than compared against
        // every stored connection.
```

The `environmentId` filter under those comments, which is only ever string-compared and never resolved — list_connections.rs:211-214:
```
                        && input
                            .environment_id
                            .as_ref()
                            .is_none_or(|wanted| connection.environment_id == *wanted)
```
(there is no `store.environments.get(..)` anywhere in this handler.)

The sibling rule it breaks — src/ops/list_attach_points.rs:154-174:
```
        // The attach points are listed FOR an Environment, so the catalog record the
        // `environmentId` names is what the listing is scoped by — and an id the catalog
        // does not hold has no Environment to list anything for. An unresolvable
        // environment is this service's own client fault — the caller named an
        // environment model that does not exist — so it is the 400
        // `InterconnectClientException` every environment-resolving operation gives,
        // never the connection-scoped 404 and never an empty page, which a caller
        // could not tell from "this environment offers none".
        let environment = state.lock().environments.get(&input.environment_id).cloned();
        let Some(environment) = environment else {
            return Err(ListAttachPointsErrorModeled::InterconnectClientException(
                InterconnectClientException {
                    message: crate::state::environment_not_found_message(&input.environment_id),
                },
            )
            .into());
        };
```
the same claim from the second sibling — src/ops/create_connection.rs:227-232:
```
        // The Environment dictates the partner, the location and the product type,
        // none of which the request carries. An unknown environment has nothing to
        // create a connection across, and naming one is this service's own client
        // fault: the 400 `InterconnectClientException` every environment-resolving
        // operation gives, with the message built from the requested id.
        let Some(environment) = store.environments.get(&input.environment_id).cloned() else {
```
and the shared refusal helper all three use — src/state.rs:1020-1028:
```
/// How the service words an `environmentId` (or `GetEnvironment$id`) that resolves to
/// no Environment.
///
/// An unresolvable environment is a CLIENT fault for this service — the caller named
/// a model that does not exist — so it is reported as `InterconnectClientException`
/// (HTTP 400), not as the connection-scoped `ResourceNotFoundException` (404), and the
/// message is built from the id the request carried.
pub fn environment_not_found_message(environment_id: &str) -> String {
```

Test coverage: the malformed-filter half of the rule is pinned by tests/all/attach_point_member_constraints.rs:521 ("the `ListConnections` `attachPoint` filter carries the same union, so it holds...") and tests/all/provider_member_constraints.rs:134-137 (both list operations refuse an out-of-length union member). The absent-environment half is pinned for the siblings by tests/all/attach_points.rs:326-365 (`unknown_environment_is_refused_rather_than_answered_empty`, which also asserts GetEnvironment agrees) and tests/all/connection_lifecycle.rs:754-768 (CreateConnection on "env-does-not-exist" is 400). No test pins the ListConnections answer as a rule; the one call that exercises it, tests/all/connection_lifecycle.rs:769-780, uses the 200/empty answer incidentally, for the stated reason "expected no connection to have been created", and would need its `ic_ok` turned into a refusal assertion by the fix.

## Steps to Reproduce

1. Boot the emulator.
2. Call ListAttachPoints with environmentId="env-does-not-exist" and observe HTTP 400 InterconnectClientException "Unable to process request: This environment model with id env-does-not-exist could not be found" (src/ops/list_attach_points.rs:166-174).
3. Call GetEnvironment with id="env-does-not-exist" and observe the same 400 refusal (src/ops/get_environment.rs:151-159).
4. Call CreateConnection with environmentId="env-does-not-exist" and observe the same 400 refusal (src/ops/create_connection.rs:232-239).
5. Call ListConnections with {"environmentId": "env-does-not-exist"} and observe HTTP 200 with "connections": [] — no refusal, and no way for the caller to distinguish an environment that does not exist from one that carries none of its connections.
6. Confirm the cause: src/ops/list_connections.rs:211-214 string-compares the filter against each stored connection and the handler never reads `store.environments`, although it holds the lock on the store that carries the catalog (list_connections.rs:198-205), and although the same handler refuses the `attachPoint` and `provider` filters up front for exactly this reason (list_connections.rs:155-187).
