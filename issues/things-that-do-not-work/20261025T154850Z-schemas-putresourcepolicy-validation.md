Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 9c887f34f86da2c6771f7b9b12ec1c8e6bf552af

# PutResourcePolicy attaches a policy to an AWS-managed registry that every sibling write refuses

_Filed: 2026-10-25T15:48:50Z_
_Operation: PutResourcePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `9c887f34f86da2c6771f7b9b12ec1c8e6bf552af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other write path in this crate resolves the registry and then refuses the call when the registry is service-owned (`RegistryScope::Aws`), answering `ForbiddenException` built by `crate::state::managed_registry_is_read_only`. `PutResourcePolicy` resolves the registry and writes `registry.policy = Some(...)` without that guard, so `PUT /v1/policy?registryName=aws.events` succeeds with HTTP 200 and persists a caller-written resource policy onto the AWS-owned catalogue registry. The crate's own state module declares the opposite rule twice: the scope enum documents it, and the seeding function writes `policy: None` with the comment "A service-owned registry carries no caller-written policy." The write is observable afterwards: `GetResourcePolicy` of `aws.events` then reports the document and revision (it has no scope check either), and `DeleteResourcePolicy` will remove it — so the AWS-owned registry permanently carries caller state that the rest of the crate says it cannot carry. `PutResourcePolicy` already models `ForbiddenException` (HTTP 403), so the shape the sibling handlers answer with is available on this operation.

## Evidence

The rule, stated in this crate's state module:

services/service-com-amazonaws-schemas-schemas/src/state.rs:204-206
```
    /// A service-owned registry, present before any call: AWS owns its contents, so a
    /// caller may read it but not write to it.
    Aws,
```

services/service-com-amazonaws-schemas-schemas/src/state.rs:2486-2487 (seeding the service-owned registries)
```
                    // A service-owned registry carries no caller-written policy.
                    policy: None,
```

services/service-com-amazonaws-schemas-schemas/src/state.rs:130-134 (`writable_tags_for_arn`)
```
    /// What a write adds is the one refusal a read does not have: what is in a
    /// service-owned registry is AWS's, so a caller may read it but not write to it —
    /// the same refusal every other write to one gets.
```

The sibling handlers that apply it — e.g. services/service-com-amazonaws-schemas-schemas/src/ops/update_registry.rs:168-175
```
        // A service-owned registry is AWS's, not the caller's, so its description is
        // not a caller's to rewrite — the same refusal every other write to one gets.
        if registry.scope.is_managed() {
            return Err(UpdateRegistryErrorModeled::ForbiddenException(
                crate::state::managed_registry_is_read_only(&input.registry_name),
            )
            .into());
        }
```
(the identical guard is at create_schema.rs:212, update_schema.rs:187, delete_schema.rs:180, delete_schema_version.rs:185, delete_registry.rs:194)

The handler that skips it — services/service-com-amazonaws-schemas-schemas/src/ops/put_resource_policy.rs:243-281
```
        // A policy attaches to a registry, so there has to be one to attach it to.
        let current_revision = state
            .registries
            .get(&registry_name)
            .ok_or_else(|| { ... NotFoundException ... })?
            .policy
            .as_ref()
            .map(|policy| policy.revision_id.clone());
...
        let registry = state
            .registries
            .get_mut(&registry_name)
            .expect("the registry read above is still present under the same lock");
        registry.policy = Some(crate::state::ResourcePolicy {
            document,
            revision_id: minted_revision.clone(),
        });
```
There is no `is_managed()` call anywhere in put_resource_policy.rs (grep for `scope.is_managed()` across `src/` lists create_schema.rs:212, update_schema.rs:187, update_registry.rs:170, delete_schema.rs:180, delete_schema_version.rs:185, delete_registry.rs:194, export_schema.rs:207 and state.rs — not this file).

Test coverage pinning the rule for the sibling write, and the absence of coverage here:
services/service-com-amazonaws-schemas-schemas/tests/all/resource_tag_removal.rs:441-480
```
/// RULE: what is in a service-owned registry is AWS's, so a caller may read it but
/// not write to it: removing a tag from one is refused 403 and the registry is still
/// there to read.
#[tokio::test]
async fn untagging_a_service_owned_registry_is_refused() {
...
            assert_eq!(
                refused.status, 403,
```
The policy suite `tests/all/resource_policy.rs` only ever writes to registries it created with `create_registry` (lines 182, 260, 349, 406, 477, 534, 587); no case names a service-owned registry, so nothing pins the current behaviour.

## Steps to Reproduce

1. Start the emulator with a fresh store (the service-owned `aws.events` registry is seeded by `crate::state::aws_managed_registries`, scope `RegistryScope::Aws`).
2. Send `PUT /v1/policy?registryName=aws.events` with body `{"Policy":"{\"Version\":\"2012-10-17\",\"Statement\":[]}"}`.
3. Observe HTTP 200 with a minted `RevisionId` instead of the 403 `ForbiddenException` every other write to that registry answers (compare `PUT /v1/registries/name/aws.events` → 403, `DELETE /tags/<aws.events ARN>?tagKeys=team` → 403).
4. Send `GET /v1/policy?registryName=aws.events` and observe the caller-written policy is now persisted on the AWS-owned registry.
