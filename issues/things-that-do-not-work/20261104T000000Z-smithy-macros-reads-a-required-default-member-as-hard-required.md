Issue-Severity: MEDIUM
Issue-Gap-Class: fidelity
Issue-Commit: mainline

# A member that is both `@required` and `@default` is read as hard-required, so a partial structure is refused where AWS applies the default

_Filed: 2026-11-04T00:00:00Z_
_Operation: cross-service (every shape with a `@required` member that also carries a usable `@default`) | Class: fidelity_

## Description

`smithy_macros::smithy(shape = "…")` decides, per member, whether an absent value is an
error or a default (`vendor/smithy-macros/src/lib.rs:2788`):

```rust
if has_required_trait || declared_default.is_none() {
    // Truly required — error if absent.
    #field_ident: #slot_ident.ok_or_else(|| missing_member(#smithy_name))?
} else {
    // `@default` but not `@required`: … AWS fills in the default server-side
    #field_ident: #slot_ident.unwrap_or_else(|| #fallback)
}
```

`has_required_trait` wins, so a member carrying BOTH traits is read as hard-required and a
request that omits it is refused before any handler runs — `1 validation error detected:
Value null at '<path>' failed to satisfy constraint: Member must not be null`.

That combination is not unusual: it is what a Smithy 1.0 shape becomes when its unboxed
primitive members are migrated, and AWS's own models carry it widely. The real service
applies the modelled default to such a member: a recorded Amazon ECS `UpdateService` that
sent `deploymentConfiguration.deploymentCircuitBreaker = {"enable": true}` — omitting the
`@required @default: false` sibling `rollback` — is answered **200**, with `rollback: false`
echoed in the response body, while the emulator answered 400.

## Evidence

* ECS: `DeploymentCircuitBreaker$enable`/`$rollback`, `DeploymentAlarms$rollback`/`$enable`,
  `ThresholdConfiguration$value` and `DeploymentEarlySuccessCriteria$enable` are each
  `@required` with a usable `@default`
  (`smithy_query(query="shape", shape="DeploymentCircuitBreaker", model_path="services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/smithy-model/com-amazonaws-ecs-amazonec2containerservicev20141113.json")`).
  Recorded step `ecs-ecsdep-traj-update-circuit-breaker-partial-struct#task#6`: AWS 200, the
  emulator 400 on the absent `rollback`.
* The same read is generated for every other service's shapes, so any recorded call that
  omits such a member is refused the same way.

## What a fix needs

The fix is in the generator, which is in the agent-immutable `vendor/` tree:
`vendor/smithy-macros/src/lib.rs:2788` should consult the DEFAULT first — a member with a
usable `@default` (`usable_default`, which already excludes `@default: null` and
`@addedDefault`) takes that default when absent, whether or not it also carries
`@required`; only a member with no usable default stays a hard error. Nothing else has to
change: the field is already non-`Option` for both cases, `default_member_fallback_expr`
already materialises the declared value, and the validating walk still sees the member
because the writer always writes a non-`Option` field.

## Workaround in the tree today

`services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/types/shared_types.rs`
holds the four shapes above in a `Defaulted<T>` wrapper whose `DeserializableStruct` starts
from the shape's modelled defaults and reads only the members the request carried, through
the shape's own GENERATED schema. It exists solely because a service crate cannot change
the generator; once the generator applies the default, those `impl DefaultedStructure`
blocks and the wrapper can be deleted and the four field types put back to the plain shape.
