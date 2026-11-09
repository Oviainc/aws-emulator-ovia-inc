Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__secretsmanager.cfn-secretsmanager-secret-ref-and-getatt-id-match-secretsmanager-arn cloudformation__secretsmanager.deploy-secretsmanager-resourcepolicy-attached-then-removed-by-stack-update cloudformation__secretsmanager.deploy-secretsmanager-rotationschedule-conflicting-rules-rejected-and-rolled-back cloudformation__secretsmanager.output-getatt-on-writeonly-secretstring-rejected
Issue-Key: cloudformation__secretsmanager.cfn-secretsmanager-secret-ref-and-getatt-id-match-secretsmanager-arn|_error#18|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cloudformation__secretsmanager.deploy-secretsmanager-resourcepolicy-attached-then-removed-by-stack-update|run#15|AssertionError|assert outputs["PolicyRef"] == secret_arn, (
Issue-Key: cloudformation__secretsmanager.deploy-secretsmanager-rotationschedule-conflicting-rules-rejected-and-rolled-back|run#7|AssertionError|assert status == "ROLLBACK_COMPLETE", f"expected ROLLBACK_COMPLETE, got {status!r}"
Issue-Key: cloudformation__secretsmanager.output-getatt-on-writeonly-secretstring-rejected|run#7|AssertionError|assert bad["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__secretsmanager: 4 net-new causes — DescribeSecret still answers for a stack-deleted secret, ResourcePolicy Ref is not the secret ARN, conflicting rotation rules accepted, GetAtt on write-only SecretString accepted

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Four distinct root causes. Each is numbered with its own failing check id; all four must be closed before this issue is done.

Cause 1 of 4: DescribeSecret keeps answering for a secret the stack deleted.
AWS: once the secret is gone, DescribeSecret is refused with `ResourceNotFoundException` and the message `Secrets Manager can't find the specified secret.`; a secret still in the recovery window carries DeletedDate and is refused for data operations.
Emulator: DescribeSecret returns HTTP 200 with the full secret description — including a DeletedDate — instead of refusing the call.
Why: a client cannot tell a live secret from a removed one, so cleanup checks and existence probes both pass when they should fail.
Fix: the SecretsManager DescribeSecret handler's lifecycle state and the AWS::SecretsManager::Secret delete path.
Traces: cloudformation__secretsmanager.cfn-secretsmanager-secret-ref-and-getatt-id-match-secretsmanager-arn.

Cause 2 of 4: Ref on AWS::SecretsManager::ResourcePolicy is a generated name instead of the secret ARN.
AWS: Ref on AWS::SecretsManager::ResourcePolicy returns the ARN of the secret the policy is attached to.
Emulator: Ref returns `<stack-name>-<logical-id>-<suffix>`.
Why: a template that passes the resource-policy Ref onward, or a client that reads it to look the secret up, gets a value SecretsManager does not accept as a SecretId.
Fix: the AWS::SecretsManager::ResourcePolicy handler's physical-resource-id/Ref resolution.
Traces: cloudformation__secretsmanager.deploy-secretsmanager-resourcepolicy-attached-then-removed-by-stack-update.

Cause 3 of 4: an AWS::SecretsManager::RotationSchedule declaring conflicting rotation rules is accepted and the stack reaches CREATE_COMPLETE.
AWS: SecretsManager refuses a rotation schedule that declares mutually exclusive rules (for example both a ScheduleExpression and AutomaticallyAfterDays) with an InvalidParameterException, so CloudFormation marks the resource CREATE_FAILED and the stack ROLLBACK_COMPLETE.
Emulator: the stack ends `CREATE_COMPLETE`.
Why: the client believes it configured rotation; the schedule that actually runs is undefined.
Fix: the AWS::SecretsManager::RotationSchedule handler's rule validation / propagation of the SecretsManager refusal.
Traces: cloudformation__secretsmanager.deploy-secretsmanager-rotationschedule-conflicting-rules-rejected-and-rolled-back.

Cause 4 of 4: Fn::GetAtt on the write-only SecretString property is accepted and the stack reaches CREATE_COMPLETE.
AWS: SecretString is a write-only property of AWS::SecretsManager::Secret; a template whose Output does Fn::GetAtt on it is refused and the stack rolls back — a secret value is never readable through a stack output.
Emulator: the stack ends `CREATE_COMPLETE` with no status reason.
Why: a write-only property is exposed through stack outputs, so secret material can leak to anyone who can read the stack.
Fix: the resource-schema write-only-property enforcement in CloudFormation's GetAtt resolution for AWS::SecretsManager::Secret.
Traces: cloudformation__secretsmanager.output-getatt-on-writeonly-secretstring-rejected.

## Evidence
Cause 1: AssertionError: expected the call to be refused, but it succeeded: {'ARN': 'arn:aws:secretsmanager:...:secret:...', 'Name': '...', 'DeletedDate': datetime(...), ... 'HTTPStatusCode': 200}. Expected ResourceNotFoundException.
Cause 2: AssertionError: the ResourcePolicy Ref was '<stack>-SecretPolicy-<suffix>', expected the secret ARN.
Cause 3: AssertionError: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE'.
Cause 4: AssertionError: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE': None.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Cause 1: deploy an AWS::SecretsManager::Secret, delete the stack, then call DescribeSecret with the ARN it reported.
3. Cause 2: deploy AWS::SecretsManager::ResourcePolicy and read its Ref.
4. Cause 3: CreateStack with an AWS::SecretsManager::RotationSchedule declaring conflicting rotation rules and watch the final stack status.
5. Cause 4: CreateStack with an Output doing Fn::GetAtt [Secret, SecretString] and watch the final stack status.
