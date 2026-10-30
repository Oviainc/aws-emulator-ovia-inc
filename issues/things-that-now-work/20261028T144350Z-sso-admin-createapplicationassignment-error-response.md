Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7c677bf184452d3016cbf464b9a60cef63f91cb5

# Re-granting an application assignment the application already holds answers an empty 200 instead of the modelled ConflictException the crate's sibling non-@idempotent writes answer

_Filed: 2026-10-28T14:43:50Z_
_Operation: CreateApplicationAssignment | Class: error-response_

> _Note: this issue was identified against the code base at commit `7c677bf184452d3016cbf464b9a60cef63f91cb5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateApplicationAssignment` is the third member of a family this crate already decided: a write the model marks NOT `@idempotent` and gives a `ConflictException` for, whose whole identity is carried by the request, refuses a request naming an identity the resource already holds rather than reporting success for a write that adds nothing. Two sibling handlers state that rule in as many words and implement it (`AttachManagedPolicyToPermissionSet`, `AttachCustomerManagedPolicyReferenceToPermissionSet`), and two more creates in the crate refuse an already-existing resource on the same channel (`CreatePermissionSet` for a taken name, `CreateInstanceAccessControlAttributeConfiguration` for a configuration that already exists).

`CreateApplicationAssignment` instead returns early with an empty 200 when the (application, principal type, principal id) row is already stored. Its own comment concedes the write adds nothing ("re-creating one that already exists grants nothing further") — which is exactly the condition the sibling rule calls "that conflict". The operation's modelled `ConflictException` variant is declared in the handler's error enum and no branch of the handler can ever construct it, so that modelled error is unreachable.

Consequence: the modelled output of this operation is EMPTY, so an empty 200 is the only answer a caller gets — it cannot tell the grant that created the access from the fifth call that created nothing, exactly the complaint the two attach siblings were changed to fix. The stored state is already right (one row per tuple); only the answer to the duplicate request diverges from the rule the crate states.

## Evidence

The rule, stated and applied by a sibling — `src/ops/attach_managed_policy_to_permission_set.rs:232-249`:
```
        // An ATTACH, not a PUT: the model marks this operation NOT `@idempotent` and
        // declares `ConflictException` for "a conflict with a previous successful write".
        // The ARN is the attachment's identity, so a request naming one the set already
        // carries is that conflict — refused, and the stored set is left as it was rather
        // than reporting success for a write that would add nothing.
        if record.managed_policies.contains(&input.managed_policy_arn) {
            return Err(
                AttachManagedPolicyToPermissionSetErrorModeled::ConflictException(
```
the same sentence in the second sibling — `src/ops/attach_customer_managed_policy_reference_to_permission_set.rs:236-244`:
```
        // An ATTACH, not a PUT: the model marks this operation NOT `@idempotent` and
        // declares `ConflictException` for "a conflict with a previous successful write".
        // The (`Name`, `Path`) pair is the attachment's identity, so a request naming one
        // the set already holds is that conflict — refused, and the stored set is left as
        // it was rather than reporting success for a write that would add nothing.
        if record.customer_managed_policies.contains_key(&identity) {
            return Err(
                AttachCustomerManagedPolicyReferenceToPermissionSetErrorModeled::ConflictException(
```
and two more creates of this crate that refuse an already-existing resource on the same channel — `src/ops/create_permission_set.rs:224-233`:
```
        // A permission set's name is unique within its instance.
        if store.permission_set_name_taken(&instance_arn, &input.name) {
            return Err(CreatePermissionSetErrorModeled::ConflictException(ConflictException {
```
`src/ops/create_instance_access_control_attribute_configuration.rs:232-240`:
```
        // A CREATE of a resource that already exists: the instance carries at most one
        // ABAC configuration, so enabling ABAC on an instance that already has one is the
        // conflict with a previous successful write rather than a silent overwrite ...
        if instance.access_control_attributes.is_some() {
            return Err(
                CreateInstanceAccessControlAttributeConfigurationErrorModeled::ConflictException(
```

The handler that breaks it — `src/ops/create_application_assignment.rs:196-208`:
```
        // (application, principal type, principal id) is the assignment's identity, so
        // re-creating one that already exists grants nothing further: it must not add a
        // second row for access the principal already holds.
        if store
            .application_assignment_position(
                &input.application_arn,
                &input.principal_type,
                &input.principal_id,
            )
            .is_some()
        {
            return Ok(CreateApplicationAssignmentOutput {});
        }
```

The modelling the sibling rule keys on holds for this operation too: `smithy_query(query="operation", operation="CreateApplicationAssignment")` lists `{"shape": "ConflictException", "errorKind": "client", "httpError": 409}` among its errors and reports NO `idempotent` flag, while the same query for `DeleteApplicationAssignment` reports `"idempotent": true` — so the model does distinguish the two here. The declared error is unreachable in the handler: `src/ops/create_application_assignment.rs:105` declares `ConflictException(ConflictException),` in `CreateApplicationAssignmentErrorModeled` and no branch of `handle` constructs it.

Test coverage: `tests/all/application_assignments.rs:739-750` pins the current answer — "// Rule 3: the same grant again grants it ONCE — answered 200, no second row." followed by `assert_eq!(status, 200, ...)` and the row-count assertion — and the reason it states is the row count ("a repeated grant must store no second row"), which a refusal preserves; no reason is stated for the 200 itself. The suite header states the same rule as "granting it again grants it once and stores no second row" (`tests/all/application_assignments.rs:17-19`). The two sibling attaches' duplicate-refusal is pinned by `tests/all/policy_attach_conflict.rs`.

## Steps to Reproduce

1. POST / with `X-Amz-Target: SWBExternalService.ListInstances` and read the account's `InstanceArn`.
2. POST / with `X-Amz-Target: SWBExternalService.CreateApplication` and body `{"InstanceArn":"<instance>","ApplicationProviderArn":"arn:aws:sso::aws:applicationProvider/custom","Name":"assign-me"}`; keep the minted `ApplicationArn`.
3. POST / with `X-Amz-Target: SWBExternalService.CreateApplicationAssignment` and body `{"ApplicationArn":"<app>","PrincipalType":"USER","PrincipalId":"c2c7c6f0-1111-2222-3333-444455556666"}` — 200 with an empty body (the grant).
4. Send the IDENTICAL request again — 200 with an empty body again, although the write added nothing and the operation declares `ConflictException` and carries no `@idempotent` trait.
5. For contrast, repeat an attach: `AttachManagedPolicyToPermissionSet` twice with the same `ManagedPolicyArn` on one permission set — the second call is refused `ConflictException`, the rule this handler skips.
6. Confirm the state is unchanged by the second grant with `DescribeApplicationAssignment` for the same tuple: one row, so the repeated call reported success for a write that did nothing.
