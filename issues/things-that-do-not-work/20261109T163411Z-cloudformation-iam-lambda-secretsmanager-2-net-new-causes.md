Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__secretsmanager.deploy-secretsmanager-rotationschedule-enabled-then-disabled-by-stack-update cloudformation__iam__lambda__secretsmanager.deploy-secretsmanager-rotationschedule-missing-invoke-permission-rejected
Issue-Key: cloudformation__iam__lambda__secretsmanager.deploy-secretsmanager-rotationschedule-enabled-then-disabled-by-stack-update|run#36|AssertionError|assert described["RotationEnabled"] is True, (
Issue-Key: cloudformation__iam__lambda__secretsmanager.deploy-secretsmanager-rotationschedule-missing-invoke-permission-rejected|run#9|AssertionError|assert status == "ROLLBACK_COMPLETE", f"expected ROLLBACK_COMPLETE, got {status!r}"
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# cloudformation__iam__lambda__secretsmanager: 2 net-new causes — AWS::SecretsManager::RotationSchedule leaves RotationEnabled false, and a rotation lambda without an invoke permission is accepted

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct root causes in the stack-driven Secrets Manager rotation surface.

Cause 1 of 2: creating an AWS::SecretsManager::RotationSchedule does not turn rotation on for the secret.
AWS: when a stack creates an AWS::SecretsManager::RotationSchedule (SecretId, RotationLambdaARN, RotationRules{AutomaticallyAfterDays|ScheduleExpression}), Secrets Manager attaches the schedule and DescribeSecret then reports RotationEnabled true together with RotationLambdaARN and RotationRules; a later stack update that removes the schedule flips RotationEnabled back to false. Smithy: secretsmanager#DescribeSecret's response carries RotationEnabled (boolean), RotationLambdaARN and RotationRules, and RotateSecret sets them — recorded behavior only for the stack-driven path.
Emulator: after the RotationSchedule resource is created, DescribeSecret reports RotationEnabled false.
Why: a client cannot tell from the service whether rotation is configured; dashboards and compliance checks that read RotationEnabled report every rotating secret as unrotated, and the disable-on-update half of the contract is unobservable.
Fix: the AWS::SecretsManager::RotationSchedule provisioner must drive the secret's rotation state (RotateSecret/CancelRotateSecret), and DescribeSecret must serialize RotationEnabled from it.
Traces: cloudformation__iam__lambda__secretsmanager.deploy-secretsmanager-rotationschedule-enabled-then-disabled-by-stack-update.

Cause 2 of 2: a RotationSchedule whose rotation function grants Secrets Manager no invoke permission is accepted.
AWS: Secrets Manager verifies it can invoke the rotation function; without a resource policy permission for secretsmanager.amazonaws.com the resource is refused, so the resource goes CREATE_FAILED and the stack ends ROLLBACK_COMPLETE. Smithy: secretsmanager#RotateSecret declares InvalidRequestException for a rotation function Secrets Manager cannot use — recorded behavior only for the stack status.
Emulator: the stack reaches CREATE_COMPLETE, so the misconfiguration is accepted.
Why: a client's deploy-time guardrail does not fire; rotation then fails silently at the first scheduled attempt, when the secret may already be relied upon.
Fix: the invoke-permission precondition in the rotation-configuration path (Secrets Manager's RotateSecret validation consulting the Lambda resource policy), surfaced through the RotationSchedule provisioner.
Traces: cloudformation__iam__lambda__secretsmanager.deploy-secretsmanager-rotationschedule-missing-invoke-permission-rejected.

## Evidence
Cause 1 — Expected: DescribeSecret.RotationEnabled is True after the RotationSchedule resource is created. Actual: AssertionError: expected RotationEnabled True after RotationSchedule create, got False
Cause 2 — Expected: stack ROLLBACK_COMPLETE. Actual: AssertionError: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE'

## Steps to Reproduce
Cause 1: CreateStack with an AWS::SecretsManager::Secret, a rotation Lambda (with the secretsmanager invoke permission) and an AWS::SecretsManager::RotationSchedule; call DescribeSecret and read RotationEnabled.
Cause 2: same stack but with no AWS::Lambda::Permission for secretsmanager.amazonaws.com; observe CREATE_COMPLETE instead of a rollback.
