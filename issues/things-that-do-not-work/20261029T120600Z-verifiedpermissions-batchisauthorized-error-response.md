Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 66b81b2d38066f623e620cf9384f94dad963db50

# The batch-size refusal runs before the policy store is resolved, so an over-long batch against a store that does not exist answers 400 instead of the 404 the crate's own precedence rule states

_Filed: 2026-10-29T12:06:00Z_
_Operation: BatchIsAuthorized | Class: error-response_

> _Note: this issue was identified against the code base at commit `66b81b2d38066f623e620cf9384f94dad963db50`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states one precedence rule for a request that is faulty in two ways: the store a `policyStoreId` names is resolved before any of the request's own payload is read, so a reference that names nothing answers `ResourceNotFoundException` whatever else is wrong. `BatchIsAuthorized`'s handler states that rule for itself right above its store lookup, and the suite that pins it covers `BatchIsAuthorized` with two payload faults (an absent `principal`, an unparseable `context.cedarJson`) — both of which are read after the lookup.

One refusal in the same handler breaks the ordering: the `requests.len() > MAX_BATCH_REQUESTS` check runs before `service_state`/`state.lock()` and therefore before `resolve_store_id`, with a comment that explicitly justifies putting it first. The list's length is part of the request's own payload, so a call carrying 31 requests against a `policyStoreId` no store has is answered 400 `ValidationException` naming `/requests`, where the rule says 404 `ResourceNotFoundException` naming `POLICY_STORE`. The answer for one unknown store therefore depends on which payload fault accompanies it. The identical inversion was already recognised as a defect on `IsAuthorized` (fixed issue 20261029T005600Z-verifiedpermissions-isauthorized-error-response) and is open on `CreatePolicyStoreAlias` and `UpdatePolicy`; this is the remaining instance on the batch authorization path.

## Evidence

The rule, stated by the suite that pins it — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/tests/all/not_found_precedes_payload_refusal.rs:7-13:
```
//! The rule under test: an operation resolves every resource the request
//! REFERENCES before it reads the request's own payload, so a reference that
//! names nothing answers the 404 `ResourceNotFoundException` whatever else the
//! request got wrong — the same answer whichever operation the reference arrived
//! on. Against a resource that DOES exist the very same payload earns its 400
//! `ValidationException`, so what is pinned here is the PRECEDENCE and not a
//! blanket 404.
```

The same rule, stated by THIS handler — src/ops/batch_is_authorized.rs:191-198:
```
        // The store is resolved BEFORE any of the request's payloads is turned
        // into a Cedar value, so a reference to a store that does not exist
        // answers `ResourceNotFoundException` rather than a refusal about the
        // payload.
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;
        let store = state.policy_stores.get(&store_id).ok_or_else(not_found)?;
```

The refusal that outranks it, 17 lines earlier — src/ops/batch_is_authorized.rs:169-183:
```
        // The model bounds `requests` below (`@length min 1`) but not above, so the
        // published batch size is refused here rather than at the boundary. It is
        // checked ahead of the store lookup: the list's own size is a property of
        // the request, not of any store, so it costs a length check rather than a
        // state read.
        if input.requests.len() > crate::state::cedar::MAX_BATCH_REQUESTS {
            return Err(invalid(crate::ops::InvalidInput::new(
                crate::ops::paths::REQUESTS,
                format!(
                    "it carries {} authorization requests, more than the {} a batch may carry",
                    input.requests.len(),
                    crate::state::cedar::MAX_BATCH_REQUESTS
                ),
            )));
        }
```
with `pub const MAX_BATCH_REQUESTS: usize = 30;` at src/state/cedar.rs:272.

The handler's sibling check for the SAME member obeys the ordering — src/ops/batch_is_authorized.rs:232-238 (`shares_principal_or_resource`, also naming `/requests`) runs after the store lookup, so the two `/requests`-level rules of one operation answer an unknown store two different ways.

The 404 being skipped is modelled: smithy_query(query="operation", operation="BatchIsAuthorized") lists `ResourceNotFoundException (HTTP 404)`, and the handler's own `not_found` builder (src/ops/batch_is_authorized.rs:160-167) exists for exactly this reference.

Test coverage: tests/all/not_found_precedes_payload_refusal.rs's `FAULTS` table has two `BatchIsAuthorized` rows (lines 188-201, fields `/requests/0/principal` and `/requests/0/context/cedarJson`) and none for an over-long `requests` list, so nothing pins or justifies the divergence.

## Steps to Reproduce

1. Boot the emulator and point a Verified Permissions client at its data listener.
2. Build a `requests` list of 31 well-formed items, each with the same principal, e.g. {principal:{entityType:"User",entityId:"dana"}, action:{actionType:"Action",actionId:"view"}, resource:{entityType:"Photo",entityId:"p<i>"}}.
3. Call BatchIsAuthorized { policyStoreId: "PSdoesnotexist11111111", requests: <the 31 items> }.
4. Observed: HTTP 400 ValidationException with fieldList naming `/requests` ("it carries 31 authorization requests, more than the 30 a batch may carry").
5. Expected per the crate's stated rule: HTTP 404 ResourceNotFoundException with resourceType `POLICY_STORE` and resourceId `PSdoesnotexist11111111` — the answer the same unknown store earns from the same operation when the payload fault is an absent `principal` instead (pinned by tests/all/not_found_precedes_payload_refusal.rs).
6. Contrast: repeat step 3 against a store that DOES exist -> the same 400 ValidationException, confirming the payload refusal itself is correct and only its precedence is wrong.
