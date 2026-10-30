Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 3ee3ad8755f7cbca1472e043fe3407ec62e0eff1

# The statement-size refusal runs before the policy store is resolved, so an over-long statement against a store that does not exist answers 400 instead of the 404 the crate's own rule states

_Filed: 2026-10-29T11:39:25Z_
_Operation: UpdatePolicy | Class: error-response_

> _Note: this issue was identified against the code base at commit `3ee3ad8755f7cbca1472e043fe3407ec62e0eff1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other handler in this crate that takes a `policyStoreId` resolves the store BEFORE it reads any of the request's own payload, so a reference that names nothing answers `ResourceNotFoundException` whatever else is wrong with the request. `UpdatePolicy` (and `CreatePolicy`, which carries the identical call) breaks that ordering for one refusal: `crate::state::cedar::check_statement_size` is called before `service_state`/`state.lock()` and therefore before `resolve_store_id`. A request whose `definition.static.statement` is longer than `MAX_STATEMENT_BYTES` and whose `policyStoreId` names no store is answered 400 `ValidationException` naming `/definition/static/statement`, where the rule (and the suite that pins it) says 404 `ResourceNotFoundException` naming `POLICY_STORE`.

`CreatePolicy`'s comment above the same call asserts the opposite of what the code does: it claims that hoisting only the length check keeps "an unknown store is still a 404". It does not — the hoisted length check is itself a payload refusal, and it now outranks the not-found. The same inversion was already recognised as a defect for `IsAuthorized` (fixed issue 20261029T005600Z-verifiedpermissions-isauthorized-error-response), so this is the remaining instance of it on the policy writers.

Note the existing open `CreatePolicy`/`error-response` issue (20261028T205535Z) is about a different ordering (quota before name conflict); this finding is about store resolution versus the statement-size check and applies to `UpdatePolicy` and `CreatePolicy` alike, both of which hoist the same helper.

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
The same rule, stated and implemented by siblings — src/ops/put_schema.rs:144-149:
```
        // The store is resolved FIRST: a schema is placed in a policy store, so a
        // reference that names no store is a missing resource whatever else the
        // request carries.
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;
```
and src/ops/create_identity_source.rs:188-194:
```
        // The store the request names is resolved BEFORE any of the request's own
        // payload is read, so a reference to a store that does not exist answers
        // `ResourceNotFoundException` rather than a refusal about the payload.
        // Resolving is a READ, so a request refused below still changes nothing.
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;
```
The path that breaks it — src/ops/update_policy.rs:236-246:
```
        // The SIZE BOUND first, before the state lock and before any Cedar parse
        // or schema validation, so no caller-sized statement reaches the parser.
        if let Some(UpdatePolicyDefinition::Static(def)) = &input.definition {
            crate::state::cedar::check_statement_size(&def.statement).map_err(invalid)?;
        }

        let state = crate::state::service_state(&*self.engine).ok_or_else(unavailable)?;
        let mut state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(store_not_found)?;
```
and the sibling whose comment contradicts itself — src/ops/create_policy.rs:244-249:
```
        // `templateLinked` one carries a template reference and two entity
        // identifiers, each already bounded by the model — so only it has a size
        // to check. Only the length is checked here; the full parse stays after
        // the store is resolved, so an unknown store is still a 404 and a client
        // token still replays before the statement is parsed.
        if let PolicyDefinition::Static(def) = &input.definition {
            crate::state::cedar::check_statement_size(&def.statement).map_err(invalid)?;
        }
```
What the hoisted check answers — src/state/cedar.rs:305-317:
```
pub fn check_statement_size(statement: &str) -> Result<(), InvalidInput> {
    if statement.len() > MAX_STATEMENT_BYTES {
        return Err(InvalidInput::new(
            paths::STATIC_STATEMENT,
```
with `pub const MAX_STATEMENT_BYTES: usize = 10_000;` at src/state/cedar.rs:89.

Both operations model the 404 that is being skipped: smithy_query(query="operation", operation="UpdatePolicy") lists `ResourceNotFoundException (HTTP 404)`, and the handler's own `store_not_found` builder (src/ops/update_policy.rs:187-191) exists for exactly this reference.

Test coverage: tests/all/not_found_precedes_payload_refusal.rs pins the precedence for `IsAuthorized`, `IsAuthorizedWithToken`, `BatchIsAuthorized`, `CreateIdentitySource` and `UpdateIdentitySource` (the `FAULTS` table, lines 96-261) but has no row for `CreatePolicy`/`UpdatePolicy`, so nothing pins or justifies the divergence; tests/all/policy_validation.rs exercises the over-long statement only against a store that exists.

## Steps to Reproduce

1. Boot the emulator and point a Verified Permissions client at its data listener.
2. Build a static Cedar statement longer than 10,000 bytes, e.g. `permit(principal, action, resource) when { "<10_000 'x' characters>" != "" };`.
3. Call UpdatePolicy { policyStoreId: "PSdoesnotexist11111111", policyId: "SPdoesnotexist11111111", definition: { static: { statement: <the over-long statement> } } }.
4. Observed: HTTP 400 ValidationException with fieldList naming `/definition/static/statement`.
5. Expected per the crate's stated rule: HTTP 404 ResourceNotFoundException with resourceType `POLICY_STORE` and resourceId `PSdoesnotexist11111111` — the answer the same reference earns from PutSchema, CreateIdentitySource, ListPolicies and every other handler for the same request.
6. Contrast: CreatePolicy { policyStoreId: "PSdoesnotexist11111111", definition: { static: { statement: <the over-long statement> } } } answers 400 as well, while CreatePolicy with a *parseable but invalid* statement against the same unknown store correctly answers 404 — so the answer for one unknown store depends on which payload fault accompanies it.
