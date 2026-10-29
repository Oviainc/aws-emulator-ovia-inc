Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 07e288609021e6571eb22a2173d293968d6da2ab

# The "a resource policy needs a RegistryName" rule is implemented three times, once per policy handler

_Filed: 2026-10-29T13:38:09Z_
_Operation: PutResourcePolicy | Class: maintainability_

> _Note: this issue was identified against the code base at commit `07e288609021e6571eb22a2173d293968d6da2ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All three resource-policy operations (`PutResourcePolicy`, `GetResourcePolicy`, `DeleteResourcePolicy`) bind `RegistryName` as an OPTIONAL `@httpQuery("registryName")`, and each handler then applies the SAME two-step rule in its own copy of the code:

1. a request that carries no `RegistryName` is refused with `crate::state::registry_name_required()` (400), and
2. the name it does carry is checked with `crate::state::validate_names(Some(&registry_name), None)`.

The rule itself lives nowhere: only its two message/constraint helpers are shared, while the branch that applies them is written out three times, in three different spellings (`input.registry_name.as_deref().ok_or_else(..)`, a destructured `registry_name.ok_or_else(..)`, and `input.registry_name.ok_or_else(..)`). The copies can drift: a fourth policy-scoped operation, or an edit to one of the three, can keep step 1 and lose step 2 (or vice versa) with nothing in the crate noticing — exactly the failure mode this crate already hoisted away twice before, for the registry write resolution (`ComAmazonawsSchemasSchemasState::writable_registry`) and the read resolution (`readable_schema_version`), both of which carry an explicit comment saying that resolving-and-refusing together is what keeps a step from being forgotten.

The fix is the same shape as those two: one `crate::state` helper that turns `Option<String>` into `Result<String, BadRequestException>` by applying both steps, with each handler mapping the single `BadRequestException` into its own modelled error enum as it already does. No response and no existing test changes — the two refusals, their `Code` and their sentences are already the service's shared constants.

## Evidence

The crate states the anti-duplication rule for exactly this kind of multi-step precondition — services/service-com-amazonaws-schemas-schemas/src/state.rs:193-199:
```
    /// Every write this service serves that changes what a registry HOLDS — the
    /// registry itself, a schema or schema version in it, its tags or its
    /// resource-based policy — takes the same two steps before it changes anything: the
    /// store must hold a registry of that name, and what is in a service-owned registry
    /// is AWS's, so a caller may read it but not write to it. Resolving and refusing
    /// together is what keeps the second step from being forgotten by a handler that
    /// remembers the first.
```
and for the name check itself — state.rs:3060-3068:
```
/// Check the caller-supplied NAMES one request carries, in the order the operation's
/// URI states them: the registry first, then the schema.
///
/// Every operation that takes one of these names routes through here — a reporting one
/// as much as a mutating one, the way both of the code-binding operations check their
/// `Language` label ([`unsupported_code_binding_language`]) — so no two of them can
/// disagree about which names this service accepts.
```

Copy 1 — services/service-com-amazonaws-schemas-schemas/src/ops/put_resource_policy.rs:187-204:
```
        // A resource policy belongs to ONE registry, so a request that names no
        // registry names no resource to write. Refused as a client error rather than
        // applied to some default resource the service does not model.
        let registry_name = registry_name.ok_or_else(|| {
            PutResourcePolicyError::from(PutResourcePolicyErrorModeled::BadRequestException(
                crate::state::registry_name_required(),
            ))
        })?;

        // And the registry it names is checked against the service's own constraint for
        // a registry name: ... See `crate::state::validate_names`.
        crate::state::validate_names(Some(&registry_name), None).map_err(|refusal| {
            PutResourcePolicyError::from(PutResourcePolicyErrorModeled::BadRequestException(
                refusal,
            ))
        })?;
```

Copy 2 — services/service-com-amazonaws-schemas-schemas/src/ops/get_resource_policy.rs:147-164:
```
        // A resource policy belongs to ONE registry, so a request that names no
        // registry names no resource to read. Same refusal as `PutResourcePolicy`.
        let registry_name = input.registry_name.as_deref().ok_or_else(|| {
            GetResourcePolicyError::from(GetResourcePolicyErrorModeled::BadRequestException(
                crate::state::registry_name_required(),
            ))
        })?;

        // The registry this request names is checked against the service's own
        // constraint for a registry name before the store is reached: ...
        crate::state::validate_names(Some(registry_name), None).map_err(|refusal| {
            GetResourcePolicyError::from(GetResourcePolicyErrorModeled::BadRequestException(
                refusal,
            ))
        })?;
```

Copy 3 — services/service-com-amazonaws-schemas-schemas/src/ops/delete_resource_policy.rs:185-203:
```
        // A resource policy belongs to ONE registry, so a request that names no
        // registry names no resource to delete. Same refusal as `PutResourcePolicy`
        // and `GetResourcePolicy`.
        let registry_name = input.registry_name.ok_or_else(|| {
            DeleteResourcePolicyError::from(DeleteResourcePolicyErrorModeled::BadRequestException(
                crate::state::registry_name_required(),
            ))
        })?;

        // The registry this request names is checked against the service's own
        // constraint for a registry name before the store is reached: ...
        crate::state::validate_names(Some(&registry_name), None).map_err(|refusal| {
            DeleteResourcePolicyError::from(DeleteResourcePolicyErrorModeled::BadRequestException(
                refusal,
            ))
        })?;
```

The three `RegistryName` members really are the same optional query binding — smithy_query(query="shape", shape="PutResourcePolicyRequest,GetResourcePolicyRequest,DeleteResourcePolicyRequest"):
```
"PutResourcePolicyRequest": {... "RegistryName": {"target": "__string", "httpQuery": "registryName", "doc": "<p>The name of the registry.</p>"} ...}
"GetResourcePolicyRequest":  {"members": {"RegistryName": {"target": "__string", "httpQuery": "registryName", ...}}}
"DeleteResourcePolicyRequest": {"members": {"RegistryName": {"target": "__string", "httpQuery": "registryName", ...}}}
```

Test coverage that pins both halves of the rule and must stay green unchanged:
* step 1 (no `RegistryName` -> 400 `BadRequestException` / `MalformedRequest`): tests/all/error_body_codes.rs:309-317 (`case: "GetResourcePolicy that names no registry"`, `uri: "/v1/policy".to_string()`, `status: 400`, `code: CODE_MALFORMED_REQUEST`) and tests/all/malformed_content_and_policy_deletion.rs:559 (`let unnamed = emulator.call("DELETE", &policy_uri(None), None).await;`).
* step 2 (a `registryName` outside the constraint -> 400): tests/all/name_constraints.rs:355-366, `every_operation_taking_a_registry_name_refuses_one_outside_the_constraint`, whose table builds `let policy = format!("/v1/policy?registryName={name}");` (line 365).
* the success paths the extraction must leave untouched: tests/all/resource_policy.rs:26-51 (`policy_uri`/`put_policy`/`get_policy`), tests/all/service_owned_policy_writes.rs:164-169, tests/all/registry_write_resolution.rs:170-177.

## Steps to Reproduce

1. Open services/service-com-amazonaws-schemas-schemas/src/ops/put_resource_policy.rs:187-204, get_resource_policy.rs:147-164 and delete_resource_policy.rs:185-203 side by side: the same two-step precondition (require `RegistryName`, then `validate_names` it) is written out three times.
2. Compare with the two precedents the crate already hoisted into `crate::state`: `ComAmazonawsSchemasSchemasState::writable_registry` (state.rs:216-227) and `readable_schema_version` (state.rs:273-292), each of which exists precisely so a handler cannot remember one step and forget the other (state.rs:193-199).
3. Delete the `validate_names` call from any ONE of the three handlers: the crate still compiles, every other policy test still passes, and only tests/all/name_constraints.rs:365's single `/v1/policy?registryName={name}` row fails — i.e. nothing structural prevents the three copies from drifting.
4. Fix: add one `crate::state` helper (e.g. `policy_registry_name(Option<String>) -> Result<String, BadRequestException>`) that performs both steps, and have all three handlers call it and map the single `BadRequestException` into their own modelled error enum. Responses, statuses, `Code` values and sentences are unchanged, so every test listed above stays green as written.
