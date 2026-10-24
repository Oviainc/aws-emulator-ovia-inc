Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 6b1652c669c36440eab65de28fac2314531b273d

# Re-attaching an AWS managed policy a permission set already holds answers an empty 200 instead of ConflictException

_Filed: 2026-10-24T20:09:00Z_
_Operation: AttachManagedPolicyToPermissionSet | Class: error-response_

> _Note: this issue was identified against the code base at commit `6b1652c669c36440eab65de28fac2314531b273d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `AttachManagedPolicyToPermissionSet` is an ATTACH, not a PUT — the model gives it no `@idempotent` trait (compare `PutApplicationAccessScope`, `PutApplicationAuthenticationMethod`, `PutApplicationGrant` and `PutPermissionsBoundaryToPermissionSet`, which the model does mark `@idempotent`) and it declares `ConflictException` ("Occurs when a conflict with a previous successful write is detected"). Attaching a `ManagedPolicyArn` the permission set already carries is exactly that conflict with a previous successful write: IAM Identity Center refuses the second attach with `ConflictException` (rendered on this awsJson1_1 surface as the 400 the crate folds every 4xx refusal to, plus `__type: ConflictException`) rather than reporting success for a write that changed nothing.

Actual: the handler treats the attachment list as an idempotent set and returns early with HTTP 200 and the empty response body when the ARN is already present, leaving the single stored entry untouched. A client that uses the refusal to discover "this policy is already on the permission set" — and tooling that reads a successful attach as "I added it" — cannot tell the first attach from the fifth.

The state shape itself is right: a permission set holds a given AWS managed policy at most once and the ARN is the attachment's identity, so only the ANSWER to the duplicate request is wrong. The fix is to refuse before the push, not to change the store.

This is the exact sibling of the already-open issue for `AttachCustomerManagedPolicyReferenceToPermissionSet` (20261024T151922Z-sso-admin-attachcustomermanagedpolicyreferencetopermissionset-error-response), whose description cites this operation's `ConflictException` on an already-attached policy as the established behaviour it mirrors — but this operation's own handler has the same unrefused duplicate, and no issue covers it.

## Evidence

Spec/model: `smithy_query(query="operation", operation="AttachManagedPolicyToPermissionSet")` lists `ConflictException` (errorKind client, httpError 409) among the operation's errors and reports no `idempotent` flag, while the same query for `PutApplicationAccessScope` reports `"idempotent": true` — the Put operations are the ones the model declares safe to repeat, the Attach operations are not. Well-established service behaviour: IAM Identity Center refuses an attach of a managed policy a permission set already carries as a conflict (the sibling-reference issue already filed for this service states the same rule and names this operation as its precedent).

Handler: services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/attach_managed_policy_to_permission_set.rs lines 234-238 — after resolving the permission set, `if record.managed_policies.contains(&input.managed_policy_arn) { return Ok(AttachManagedPolicyToPermissionSetOutput {}); }` with the comment "Already attached: the ARN is the attachment's identity, so attaching it again attaches it once". No branch constructs `ConflictException`, so that modelled error is unreachable for this operation.

Test coverage: no test asserts a refusal for a duplicate attach. services/service-com-amazonaws-ssoadmin-swbexternalservice/tests/all/managed_policies_in_permission_set.rs and managed_policies.rs exercise repeated attaches as successes, so that expectation changes with the handler.

## Steps to Reproduce

1. Start the emulator and read the account's instance ARN with `X-Amz-Target: SWBExternalService.ListInstances`.
2. Enable the permission-set surface on that instance if it is an account instance: POST / with `X-Amz-Target: SWBExternalService.UpdateInstance` and body `{"InstanceArn":"<instance>","PermissionSetsEnabled":true}`.
3. Create a permission set: `X-Amz-Target: SWBExternalService.CreatePermissionSet` with `{"InstanceArn":"<instance>","Name":"MpSet"}` and keep the returned `PermissionSetArn`.
4. Attach a managed policy: `X-Amz-Target: SWBExternalService.AttachManagedPolicyToPermissionSet` with `{"InstanceArn":"<instance>","PermissionSetArn":"<ps>","ManagedPolicyArn":"arn:aws:iam::aws:policy/ReadOnlyAccess"}` — 200, empty body (correct).
5. Send the IDENTICAL request again: the emulator answers 200 with an empty body, where AWS refuses the already-attached policy with `ConflictException` (HTTP 400 on this awsJson1_1 surface, `__type: ConflictException`).
6. Confirm nothing changed with `ListManagedPoliciesInPermissionSet`: one `AttachedManagedPolicy`, so the second call reported success for a write that did nothing.
