Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: d0484a4a393e00c66ab2f2d9da27b0764a19da5b

# IsAuthorized refuses the request payload before resolving the policy store, inverting the precedence its BatchIsAuthorized sibling states and implements

_Filed: 2026-10-29T00:56:00Z_
_Operation: IsAuthorized | Class: error-response_

> _Note: this issue was identified against the code base at commit `d0484a4a393e00c66ab2f2d9da27b0764a19da5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BatchIsAuthorized` resolves `policyStoreId` while holding the state lock BEFORE it turns any request payload into a Cedar value, and its code says why: a reference to a store that does not exist must answer `ResourceNotFoundException` "rather than a refusal about the payload". `IsAuthorized` does the opposite — it checks the triple's presence and converts `context`/`entities` first, and only then takes the lock and resolves the store — so one and the same pair of faults (an unknown policy store PLUS an absent `action` or an unparseable `context`) is answered 404 by the batch operation and 400 `ValidationException` by the single-request one. `IsAuthorizedWithToken` follows `IsAuthorized`'s order too (token-presence check, `resource`/`action` checks and the `context`/`entities` conversions all run before the lock at line 246 and the store resolution at line 252), so a token-less call against a store that does not exist is also a 400 there. Nothing in the crate states a reason for the single-request handlers to invert the precedence their sibling fixes.

## Evidence

Sibling rule, stated and implemented — src/ops/batch_is_authorized.rs:191-198:
```
        // The store is resolved BEFORE any of the request's payloads is turned
        // into a Cedar value, so a reference to a store that does not exist
        // answers `ResourceNotFoundException` rather than a refusal about the
        // payload.
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;
```
(the per-request member checks and `context_to_cedar_json` follow at batch_is_authorized.rs:204-230).

This handler does it the other way round — src/ops/is_authorized.rs:174-199:
```
        let principal = crate::state::cedar::request_entity(input.principal.as_ref())
            .ok_or_else(|| required(crate::ops::paths::PRINCIPAL))?;
...
        let entities = crate::state::cedar::entities_to_cedar_json(input.entities.as_ref())
            .map_err(invalid)?;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| { ... })?;
        let state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;
```
Same inversion in src/ops/is_authorized_with_token.rs:215-254 (`presented.is_empty()` refusal at 215-220, `resource`/`action` at 231-234, `context`/`entities` at 238-244, state lock at 251, `resolve_store_id` at 252-254).

Test coverage: tests/all/policy_store_authorization.rs:894-920 exercises an unknown store on IsAuthorized only with a COMPLETE, well-formed triple (`"policyStoreId": "PSnope"` plus principal/action/resource) and asserts 404, so no test pins the 400-first behaviour for a request that is faulty in both ways.

## Steps to Reproduce

1. Call IsAuthorized with `{"policyStoreId": "PSnope", "principal": {"entityType":"User","entityId":"dana"}, "resource": {"entityType":"Photo","entityId":"p"}}` (no `action`), where no policy store `PSnope` exists.
2. Observe HTTP 400 `ValidationException` with `fieldList[0].path == "/action"`.
3. Call BatchIsAuthorized with `{"policyStoreId": "PSnope", "requests": [{"principal": {"entityType":"User","entityId":"dana"}, "resource": {"entityType":"Photo","entityId":"p"}}]}` — the same two faults.
4. Observe HTTP 404 `ResourceNotFoundException`, the precedence batch_is_authorized.rs:191-194 states.
5. Repeat with a well-formed triple but `"context": {"cedarJson": "not-json"}` against `PSnope`: IsAuthorized answers 400, BatchIsAuthorized answers 404.
