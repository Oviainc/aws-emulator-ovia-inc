Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__logs.logs-loggroup-handler-permission-role-deploys-then-denies-without-create-permission cloudformation__iam__logs.logs-loggroup-handler-permission-update-denied-then-delete-denied-recovers
Issue-Key: cloudformation__iam__logs.logs-loggroup-handler-permission-role-deploys-then-denies-without-create-permission|run#21|AssertionError|assert status == "CREATE_FAILED", f"the role missing logs:CreateLogGroup produced {status!r}"
Issue-Key: cloudformation__iam__logs.logs-loggroup-handler-permission-update-denied-then-delete-denied-recovers|run#38|AssertionError|assert status == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# CloudFormation Logs resource handlers perform no IAM authorization: a role without logs:CreateLogGroup or logs:PutRetentionPolicy still reaches CREATE_COMPLETE / UPDATE_COMPLETE

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the resource handler ignores the role the stack is deployed with.
AWS: a stack deployed with a RoleARN whose policy omits logs:CreateLogGroup fails the AWS::Logs::LogGroup create with an AccessDenied resource-handler reason and the stack ends ROLLBACK_COMPLETE; an update that needs logs:PutRetentionPolicy without that permission fails and the stack ends UPDATE_ROLLBACK_COMPLETE, and a delete without logs:DeleteLogGroup fails likewise. The recorded run shows the deploy denied and then succeeding once the permission is granted.
Emulator: the permission-less deploy reaches 'CREATE_COMPLETE' and the permission-less update reaches 'UPDATE_COMPLETE' with no failed resource rows — the handler never evaluates the deployment role's policy.
Why: a client cannot validate least-privilege deployment roles; a pipeline that should be blocked silently creates and mutates resources, and the recovery path (grant, retry) is untestable.
Fix: resource-handler authorization in the CloudFormation crate — each AWS::Logs::* handler must make its service call under the stack's RoleARN and surface the resulting AccessDenied as a CREATE_FAILED/UPDATE_FAILED reason.
Traces: cloudformation__iam__logs.logs-loggroup-handler-permission-role-deploys-then-denies-without-create-permission, cloudformation__iam__logs.logs-loggroup-handler-permission-update-denied-then-delete-denied-recovers.

## Evidence
Expected: the deploy/update is denied for the under-privileged role. Actual: 'AssertionError: the role missing logs:CreateLogGroup produced 'CREATE_COMPLETE'' and 'AssertionError: the role missing logs:PutRetentionPolicy produced 'UPDATE_COMPLETE': []'.

## Steps to Reproduce
Replay cloudformation__iam__logs.logs-loggroup-handler-permission-role-deploys-then-denies-without-create-permission and .logs-loggroup-handler-permission-update-denied-then-delete-denied-recovers: deploy/update an AWS::Logs::LogGroup stack with a RoleARN lacking the needed logs permission.
