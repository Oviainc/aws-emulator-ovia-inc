Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# Re-attaching a customer managed policy reference a permission set already holds answers an empty 200 instead of ConflictException

_Filed: 2026-10-24T15:19:22Z_
_Operation: AttachCustomerManagedPolicyReferenceToPermissionSet | Class: error-response_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `AttachCustomerManagedPolicyReferenceToPermissionSet` is an ATTACH, not a PUT — the model gives it no `@idempotent` trait (compare `PutApplicationAccessScope`, `PutApplicationAuthenticationMethod` and `PutPermissionsBoundaryToPermissionSet`, which the model does mark `@idempotent`) and it declares `ConflictException` ("Occurs when a conflict with a previous successful write is detected"). Attaching a (`Name`, `Path`) reference that the permission set already carries is exactly that conflict with a previous successful write: IAM Identity Center refuses the second attach with `ConflictException` (the wire status this service answers every 4xx refusal with, 400, plus `__type: ConflictException`) rather than reporting success for a write that changed nothing. This mirrors the sibling attach operation, `AttachManagedPolicyToPermissionSet`, which refuses a policy already attached to the permission set with `ConflictException`.

Actual: the handler treats the attachment set as an idempotent map insert, so a second attach of the same reference answers HTTP 200 with the empty response body and leaves the single stored entry untouched. A client that uses the refusal to discover "this policy reference is already on the permission set" (and tools that treat a successful attach as "I added it") cannot tell the first attach from the fifth.

Note the state shape itself is right — a reference's identity is its (`Name`, `Path`) pair and the permission set must hold it at most once; only the ANSWER to the duplicate request is wrong, so the fix is to refuse before the insert, not to change the store.

## Evidence

Spec/model: `smithy_query(query="operation", operation="AttachCustomerManagedPolicyReferenceToPermissionSet")` lists `ConflictException` (errorKind client, httpError 409) among its errors and reports NO `idempotent` flag on the operation, while the same query for `PutApplicationAccessScope` reports `"idempotent": true`. The Put operations are the ones the model declares safe to repeat; the Attach operations are not. Well-established service behaviour: the sibling `AttachManagedPolicyToPermissionSet` answers a policy that is already attached to the permission set with `ConflictException`.

Handler: services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/attach_customer_managed_policy_reference_to_permission_set.rs — after resolving the permission set (lines 218-232) the only check is the emulator's own per-permission-set ceiling (lines 234-252), and line 255 does `record.customer_managed_policies.insert(identity, reference);` with the comment "A SET keyed by the reference's identity: attaching the same (Name, Path) pair twice attaches it once." No branch inspects whether the key was already present, so `ConflictException` is unreachable for this operation.

Test coverage: no test asserts a refusal. The opposite is pinned — services/service-com-amazonaws-ssoadmin-swbexternalservice/tests/all/customer_managed_policy_references.rs states the rule under test as "`AttachCustomerManagedPolicyReferenceToPermissionSet` adds one entry to that set (so attaching the same pair twice attaches it once)" (file header, lines 9-10) and exercises repeated attaches as successes, so that expectation has to change with the handler.

## Steps to Reproduce

1. Start the emulator and read the account's instance ARN with `X-Amz-Target: SWBExternalService.ListInstances`.
2. Enable the permission-set surface on that instance: POST / with `X-Amz-Target: SWBExternalService.UpdateInstance` and body `{"InstanceArn":"<instance>","PermissionSetsEnabled":true}`.
3. Create a permission set: `X-Amz-Target: SWBExternalService.CreatePermissionSet` with `{"InstanceArn":"<instance>","Name":"CmpSet"}` and keep the returned `PermissionSetArn`.
4. Attach a reference: `X-Amz-Target: SWBExternalService.AttachCustomerManagedPolicyReferenceToPermissionSet` with `{"InstanceArn":"<instance>","PermissionSetArn":"<ps>","CustomerManagedPolicyReference":{"Name":"ReadOnlyPolicy","Path":"/"}}` — 200, empty body (correct).
5. Send the IDENTICAL request again and observe another 200 with an empty body, where AWS refuses the already-attached reference with a `ConflictException` (HTTP 400 on this awsJson1_1 surface, `__type: ConflictException`).
6. Confirm nothing changed in state with `ListCustomerManagedPolicyReferencesInPermissionSet`: one entry, so the second call reported success for a write that did nothing.
7. Repeat step 5 with `Path` omitted (which normalises to `/`) to see the same unrefused duplicate through the defaulted path.
