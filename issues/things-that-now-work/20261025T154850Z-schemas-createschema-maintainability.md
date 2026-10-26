Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9c887f34f86da2c6771f7b9b12ec1c8e6bf552af

# The service-owned-registry write refusal is open-coded in six handlers, and has already drifted

_Filed: 2026-10-25T15:48:50Z_
_Operation: CreateSchema | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9c887f34f86da2c6771f7b9b12ec1c8e6bf552af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a write to a registry whose scope is `RegistryScope::Aws` is refused 403 with `managed_registry_is_read_only`" is implemented as six byte-identical guard blocks, one per handler, instead of once beside the registry resolution it always follows. The copies can drift, and they already have: `PutResourcePolicy` resolves a registry and writes to it without the guard (see the companion issue). Consolidating the resolve-then-refuse step into one helper in `crate::state` (which already owns the equivalent helper for tag writes, `writable_tags_for_arn`, returning `ResourceArnWriteError::ServiceOwned`) would leave every response and every existing test unchanged, since each call site only maps the refusal into its own operation's error enum.

## Evidence

Six identical copies of the same branch, each differing only in the error-enum name:

services/service-com-amazonaws-schemas-schemas/src/ops/create_schema.rs:211-217
```
        // AWS owns what is in a service-owned registry, so a caller cannot add to it.
        if registry.scope.is_managed() {
            return Err(CreateSchemaErrorModeled::ForbiddenException(
                crate::state::managed_registry_is_read_only(&input.registry_name),
            )
            .into());
        }
```
services/service-com-amazonaws-schemas-schemas/src/ops/update_schema.rs:185-192
```
        // AWS owns the schemas in a service-owned registry, so a caller cannot write
        // a version of one.
        if registry.scope.is_managed() {
            return Err(UpdateSchemaErrorModeled::ForbiddenException(
                crate::state::managed_registry_is_read_only(&input.registry_name),
            )
            .into());
        }
```
services/service-com-amazonaws-schemas-schemas/src/ops/delete_schema.rs:178-185, delete_schema_version.rs:183-190, delete_registry.rs:192-199 and update_registry.rs:168-175 carry the same block verbatim (grep for `scope.is_managed()` in `src/` lists exactly these six handler sites).

The state module already owns the same rule as a single helper for the tag path — services/service-com-amazonaws-schemas-schemas/src/state.rs:152-160
```
        if let Some(registry) = owning_registry {
            if self
                .registries
                .get(&registry)
                .is_some_and(|held| held.scope.is_managed())
            {
                return Err(ResourceArnWriteError::ServiceOwned(registry));
            }
        }
```
and `UntagResource` only maps that one refusal into its own enum (services/service-com-amazonaws-schemas-schemas/src/ops/untag_resource.rs:216-220), which is the shape the six copies could take.

Existing tests that pin the behaviour and must stay green unchanged: tests/all/resource_tag_removal.rs:445 `untagging_a_service_owned_registry_is_refused`, tests/all/schema_version_deletion.rs:384-412 (managed versions survive a delete attempt), tests/all/aws_managed_and_versions.rs:413 and :495 (managed registry/schema write refusals), tests/all/error_body_codes.rs:94 (`CODE_FORBIDDEN` body for a managed-registry write).

## Steps to Reproduce

1. grep -n 'scope.is_managed()' services/service-com-amazonaws-schemas-schemas/src — six handler copies of the same guard appear (create_schema.rs:212, update_schema.rs:187, update_registry.rs:170, delete_schema.rs:180, delete_schema_version.rs:185, delete_registry.rs:194).
2. Compare each block: the condition, the `ForbiddenException` variant and the `crate::state::managed_registry_is_read_only(&input.registry_name)` argument are identical; only the enum type name differs.
3. Note that put_resource_policy.rs performs the same registry resolution (lines 243-281) and carries no copy of the guard, which is the drift the duplication allows.
4. Run the crate's test suite before and after folding the resolve-then-refuse step into one `crate::state` helper; every assertion above must pass unchanged.
