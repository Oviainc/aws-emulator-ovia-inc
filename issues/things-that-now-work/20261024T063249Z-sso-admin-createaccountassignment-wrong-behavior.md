Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# CreateAccountAssignment answers with a terminal SUCCEEDED status instead of the in-flight IN_PROGRESS the operation is documented to be polled for

_Filed: 2026-10-24T06:32:48Z_
_Operation: CreateAccountAssignment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler applies the assignment synchronously and answers with `AccountAssignmentCreationStatus.Status = "SUCCEEDED"`, additionally populating every detail member (TargetId, TargetType, PermissionSetArn, PrincipalType, PrincipalId) in the create response.

Expected: IAM Identity Center creates an account assignment ASYNCHRONOUSLY. The operation's own documentation says "After a successful response, call DescribeAccountAssignmentCreationStatus to describe the status of an assignment creation request" — the create returns a tracking record whose `Status` is the non-terminal `IN_PROGRESS` (one of `StatusValues` = IN_PROGRESS | FAILED | SUCCEEDED) together with `RequestId` and `CreatedDate`, and the operation only reaches `SUCCEEDED` later, as observed through the status-describe/list operations. This is the same shape of behavior the crate already models elsewhere for instance activation and ABAC enablement (a stored due instant plus `materialize_due`), so it can be expressed without sleeping.

Actual: the first and only response already claims terminal success, so a client that polls for the transition never sees one, and a client comparing the create response against the real service sees a different Status value (and extra detail members) in the body.

## Evidence

Spec: `CreateAccountAssignment` output member `AccountAssignmentCreationStatus` targets `AccountAssignmentOperationStatus` whose `Status` targets `StatusValues` = [IN_PROGRESS, FAILED, SUCCEEDED]; the operation doc (model, rendered in the handler header lines 52-61) instructs the caller to poll `DescribeAccountAssignmentCreationStatus` after a successful response, which is only meaningful for a non-terminal initial status.

Handler: `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/create_account_assignment.rs` lines 320-334 store `AssignmentOperationRecord { status: crate::state::STATUS_SUCCEEDED, ... }` (state.rs `STATUS_SUCCEEDED = "SUCCEEDED"`) and answer from it; the module header states the choice explicitly ("the status it reports is the terminal SUCCEEDED rather than a pending one"). `AssignmentOperationRecord::to_shape` (state.rs ~line 763) fills in every detail member of the status shape.

Test coverage: `tests/all/ssoadmin_permission_sets.rs:945-960` asserts `Status == "SUCCEEDED"` on the create response, i.e. the current behavior is pinned by a test and no test asserts the in-flight status; the recorded trace for this operation is the account-instance ValidationException refusal (see tests/all/account_instance_restriction.rs header), so no recording covers the success body.

## Steps to Reproduce

1. Boot the emulator; take the account's instance ARN from SWBExternalService.ListInstances and enable permission sets on it with SWBExternalService.UpdateInstance {"InstanceArn":<arn>,"PermissionSetsEnabled":true}.
2. Call SWBExternalService.CreatePermissionSet {"InstanceArn":<arn>,"Name":"RealSet"} and keep the returned PermissionSetArn.
3. Call SWBExternalService.CreateAccountAssignment {"InstanceArn":<arn>,"TargetId":"123456789012","TargetType":"AWS_ACCOUNT","PermissionSetArn":<ps-arn>,"PrincipalType":"USER","PrincipalId":"f81d4fae-7dec-11d0-a765-00a0c91e6bf6"}.
4. Observe AccountAssignmentCreationStatus.Status == "SUCCEEDED" in the immediate response (plus TargetId/TargetType/PermissionSetArn/PrincipalType/PrincipalId echoed), where the real service answers Status "IN_PROGRESS" with RequestId/CreatedDate and only later reports SUCCEEDED to a poller.
