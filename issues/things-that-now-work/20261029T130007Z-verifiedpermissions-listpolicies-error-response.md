Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 4cfc65c0defa14c543cb5de0c0380dc37d9ef00f

# The three paginated store listings judge nextToken before resolving the policy store, inverting the precedence the crate's own suite states

_Filed: 2026-10-29T13:00:06Z_
_Operation: ListPolicies | Class: error-response_

> _Note: this issue was identified against the code base at commit `4cfc65c0defa14c543cb5de0c0380dc37d9ef00f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListPolicies`, `ListIdentitySources` and `ListPolicyTemplates` each decode (and refuse) the `nextToken` member BEFORE they reach state and resolve `policyStoreId`. A request that is faulty in two ways at once — a `policyStoreId` that names no store or no active alias AND a cursor this service never minted — is therefore answered 400 `ValidationException` naming `/nextToken`, where this crate's stated rule (and every other handler that takes a `policyStoreId`) answers 404 `ResourceNotFoundException` naming the missing `POLICY_STORE` / `POLICY_STORE_ALIAS`.

The rule is not inferred: the crate's own suite `tests/all/not_found_precedes_payload_refusal.rs` exists to pin it, and the sibling handlers state it in comments right above the resolve (`put_schema.rs`, `create_identity_source.rs`, `is_authorized.rs`). The three list handlers are the remaining instances of the inversion: the same inversion was already recognised as a defect for `IsAuthorized` (fixed), and for `BatchIsAuthorized`, `CreatePolicyStoreAlias` and `UpdatePolicy`/`CreatePolicy` (open) — none of those cover the list operations, whose refusal is the cursor rather than a Cedar payload.

All three handlers carry the identical hoisted block, so all three must move the decode after `resolve_store_id` (the decode needs no state, so it can simply follow the lookup). The consequence is that the answer for one unknown store depends on whether a cursor happened to accompany the request: a client paging a store that was deleted mid-walk is told its cursor is bad instead of that the store is gone, which sends it to re-mint a token against a store that no longer exists.

## Evidence

The rule, stated by the suite that exists to pin it — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/tests/all/not_found_precedes_payload_refusal.rs:7-13:
```
//! The rule under test: an operation resolves every resource the request
//! REFERENCES before it reads the request's own payload, so a reference that
//! names nothing answers the 404 `ResourceNotFoundException` whatever else the
//! request got wrong — the same answer whichever operation the reference arrived
//! on. Against a resource that DOES exist the very same payload earns its 400
//! `ValidationException`, so what is pinned here is the PRECEDENCE and not a
//! blanket 404.
```
The same rule stated and implemented by siblings — src/ops/put_schema.rs:144-149:
```
        // The store is resolved FIRST: a schema is placed in a policy store, so a
        // reference that names no store is a missing resource whatever else the
        // request carries.
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;
```
and src/ops/is_authorized.rs:177-184:
```
        // The store the request names is resolved BEFORE any of the request's own
        // payload is read, so a reference to a store that does not exist answers
        // `ResourceNotFoundException` rather than a refusal about the payload —
        // the one precedence every handler of a `policyStoreId` follows, and the
        // one `BatchIsAuthorized` states.
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
```
The path that breaks it — src/ops/list_policies.rs:149-172 (the payload refusal runs first, before `service_state`/`lock`/`resolve_store_id`):
```
        // A cursor this service did not mint is a bad request, not an empty page:
        // answering an empty page would hide the client's mistake.
        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(crate::state::decode_page_token(token).ok_or_else(|| {
                ListPoliciesErrorModeled::ValidationException(crate::ops::validation_exception([
                    crate::ops::InvalidInput::new(
                        crate::ops::paths::NEXT_TOKEN,
                        "it is not a cursor this service issued",
                    ),
                ]))
            })?),
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| { ... })?;
        let state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;
```
The identical hoisted block in the two other store-scoped listings — src/ops/list_identity_sources.rs:152-175 (`decode_page_token` at :156, `resolve_store_id` at :172-174) and src/ops/list_policy_templates.rs:147-170 (`decode_page_token` at :151, `resolve_store_id` at :167-169).

Both operations model the 404 that is skipped: OPERATIONS.md:198 (`ListPolicies` — `ResourceNotFoundException (HTTP 404)`), :163 (`ListIdentitySources`), :233 (`ListPolicyTemplates`), and each handler already owns the builder for it (`list_policies.rs:143-147`).

Test coverage: nothing pins the combination. tests/all/list_identity_sources.rs:495-536 sends the unknown store/alias WITHOUT a token (expects 404), and :541-566 sends the bad cursors against the LIVE store (expects 400) — the two faults are never combined, and `not_found_precedes_payload_refusal.rs`'s FAULTS table (lines 96-261) has no row for any list operation, so no test or comment justifies the divergence.

## Steps to Reproduce

1. Boot the emulator and point a Verified Permissions client at its data listener.
2. Call ListPolicies { "policyStoreId": "PSdoesnotexist11111111", "nextToken": "notacursor" } (the token satisfies NextToken's modelled pattern/length, so the boundary validator admits it).
3. Observed: HTTP 400 ValidationException with fieldList[0].path = "/nextToken".
4. Expected per the crate's stated rule: HTTP 404 ResourceNotFoundException with resourceType "POLICY_STORE" and resourceId "PSdoesnotexist11111111" — the answer the same reference earns from the same call without a nextToken (tests/all/list_identity_sources.rs:495-536), and from PutSchema/CreateIdentitySource/IsAuthorized for a request that is also faulty in its payload.
5. Repeat with ListIdentitySources and ListPolicyTemplates, and with the alias form "policy-store-alias/nope" (expected resourceType POLICY_STORE_ALIAS): all three answer 400 instead of 404.
6. Contrast: the same unknown store with a VALID-looking cursor (e.g. nextToken "7a7a") answers 404 correctly, so the answer for one unknown store depends on which cursor accompanied it.
