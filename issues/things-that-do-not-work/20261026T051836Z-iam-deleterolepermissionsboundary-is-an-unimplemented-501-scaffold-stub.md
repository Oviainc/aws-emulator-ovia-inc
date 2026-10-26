Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.service-linked-role-unmodifiable-enforced
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.service-linked-role-unmodifiable-enforced|run#16|AssertionError|assert code_delete_boundary == "UnmodifiableEntity", (
Issue-Commit: f06e2d80cab93dbbf218eecfe39480d52f3987c2

# iam DeleteRolePermissionsBoundary is an unimplemented 501 scaffold stub, so a service-linked role's permissions-boundary protection is never enforced

> _Note: this issue was identified against the code base at commit `f06e2d80cab93dbbf218eecfe39480d52f3987c2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRolePermissionsBoundary

## Description
One net-new root cause in the bootstrapped `iam` service. The failing check is marked neither ALREADY FILED nor OWNED in the replay report (the open issue 20261026T042432Z-iam-2-net-new-root-causes-updaterole-updateroledescription.md lists this check id, but its replay key is `iam|UpdateRole|UnmodifiableEntity`; UpdateRole/UpdateRoleDescription now succeed on a service-linked role, so the check runs past that leg and fails further on at a different operation. That issue explicitly records that the protected-path legs were 'never reached ... so their text is unverified by this replay' — this is that unverified leg, now reached and diverging).

Cause 1 of 1: DeleteRolePermissionsBoundary is a scaffolded, unimplemented operation and answers HTTP 501 `NotImplemented` for every request, instead of either succeeding on an ordinary role or refusing a service-linked role with UnmodifiableEntity.

AWS: Smithy DeleteRolePermissionsBoundary takes a single member, RoleName (roleNameType, required, @length min 1 max 64, pattern ^[\w+=,.@-]+$), and returns an empty body (output shape smithy.api#Unit, HTTP 200). Its modeled errors are NoSuchEntityException (awsQueryError code `NoSuchEntity`, HTTP 404), ServiceFailureException (`ServiceFailure`, HTTP 500) and UnmodifiableEntityException (`UnmodifiableEntity`, HTTP 400). The operation's own model documentation states "Deletes the permissions boundary for the specified IAM role. You cannot set the boundary for a service-linked role.", and UnmodifiableEntityException's documentation states "The request was rejected because service-linked roles are protected Amazon Web Services resources. Only the service that depends on the service-linked role can modify or delete the role on your behalf." So against a role whose path is /aws-service-role/<servicePrincipal>/ the call is refused HTTP 400 with error code `UnmodifiableEntity`; the recorded message for this protected-role class is "Cannot perform the operation on the protected role '<roleName>' - this role is only modifiable by AWS" — every word of that is fixed text except <roleName>, which is the RoleName taken from the request (here the AWSServiceRoleForAutoScaling_<CustomSuffix> name CreateServiceLinkedRole minted). Against a role that exists and is not service-linked the call returns HTTP 200 with an empty body whether or not a boundary was attached; against a RoleName that does not exist it is HTTP 404 `NoSuchEntity`.

Emulator: every DeleteRolePermissionsBoundary call is refused HTTP 501, error code `NotImplemented`, message "operation DeleteRolePermissionsBoundary is scaffolded but not implemented in this emulator". The handler's `handle` body is unconditionally `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteRolePermissionsBoundary"))`, so no input is inspected at all: the role is never looked up, the service-linked path is never recognised, and neither the 200, the 404 nor the 400 branch exists.

Why: a client cannot remove a permissions boundary from any IAM role in this emulator, so the documented way to widen a role back out after boundary-scoped delegation simply fails. Worse for correctness-sensitive callers, the refusal carries the wrong status and code: tooling that distinguishes "this role is AWS-managed, skip it" (400 `UnmodifiableEntity`) from "this role does not exist" (404 `NoSuchEntity`) from "the boundary is gone" (200) gets a single 501 `NotImplemented` for all three, which no AWS SDK error-handling branch models. Guardrail logic that asserts an SLR's boundary cannot be stripped reads the 501 as an unclassified server fault rather than as the protection holding, so a reviewer cannot tell a protected role from a broken endpoint.

Fix: the operation's handler, services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/delete_role_permissions_boundary.rs — replace the `not_implemented` return with real behaviour over the role store (resolve RoleName; 404 NoSuchEntityException when absent; 400 UnmodifiableEntityException with AWS's constant protected-role text when the role is service-linked; otherwise clear the stored boundary and return the empty 200 output) and delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker on line 4. The same protected-role constant is shared with src/ops/delete_role.rs, whose SLR refusal the check asserts immediately afterwards and which this replay still has not reached.

Traces: iam.service-linked-role-unmodifiable-enforced.

## Evidence
Check iam.service-linked-role-unmodifiable-enforced (verdict: mismatch):

  File "check_iam_service_linked_role_unmodifiable_enforced.py", line 104, in run
    assert code_delete_boundary == "UnmodifiableEntity", (
  AssertionError: DeleteRolePermissionsBoundary on a service-linked role: expected UnmodifiableEntity, got NotImplemented

Expected (AWS): HTTP 400, Error.Code `UnmodifiableEntity`, Error.Message "Cannot perform the operation on the protected role '<roleName>' - this role is only modifiable by AWS" — fixed text apart from <roleName>, which is the RoleName from the request (the AWSServiceRoleForAutoScaling_<CustomSuffix> name returned by CreateServiceLinkedRole; the generated suffix in any one run is not part of the requirement).

Actual (emulator): HTTP 501, Error.Code `NotImplemented`, Error.Message "operation DeleteRolePermissionsBoundary is scaffolded but not implemented in this emulator".

Source confirming the cause: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/delete_role_permissions_boundary.rs line 4 carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and line 179 is the unconditional `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteRolePermissionsBoundary"))`.

Still unverified by this replay (blocked behind the above, not filed as a separate cause): the plain DeleteRole leg against the same service-linked role, which AWS refuses 400 `UnmodifiableEntity` with the same constant message, and the managed DeleteServiceLinkedRole / GetServiceLinkedRoleDeletionStatus workflow that follows it.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite; check iam.service-linked-role-unmodifiable-enforced fails.
2. Minimal repro — iam CreateServiceLinkedRole(AWSServiceName='autoscaling.amazonaws.com', CustomSuffix='<unique>', Description='x'); note the returned Role.RoleName.
3. Call iam DeleteRolePermissionsBoundary(RoleName=<that RoleName>). Observe HTTP 501 / `NotImplemented` instead of HTTP 400 / `UnmodifiableEntity` with message "Cannot perform the operation on the protected role '<roleName>' - this role is only modifiable by AWS".
4. Also call iam CreateRole(RoleName='<ordinary-role>', AssumeRolePolicyDocument=<any valid trust policy>) then DeleteRolePermissionsBoundary(RoleName='<ordinary-role>'): observe the same 501 where AWS returns HTTP 200 with an empty body. And DeleteRolePermissionsBoundary(RoleName='<name-that-does-not-exist>'): observe 501 where AWS returns HTTP 404 / `NoSuchEntity`.
5. Re-replay iam.service-linked-role-unmodifiable-enforced to confirm the fix (it then proceeds to the DeleteRole and DeleteServiceLinkedRole legs, which this replay has never reached).
