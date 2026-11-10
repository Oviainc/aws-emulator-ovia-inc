Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__secretsmanager__ssm.cloudformation-stack-deletion-removes-the-secret-and-the-ssm-reference-then-404s
Issue-Key: cloudformation__secretsmanager__ssm.cloudformation-stack-deletion-removes-the-secret-and-the-ssm-reference-then-404s|_error#16|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 0c065617979d1ee76fa77b423bd58183fc04c861

# cloudformation__secretsmanager__ssm: a stack-deleted AWS::SecretsManager::Secret is only scheduled for deletion — DescribeSecret still answers 200 instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `0c065617979d1ee76fa77b423bd58183fc04c861`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecret

## Description
One net-new root cause.

Cause 1 of 1: deleting the stack that provisioned an AWS::SecretsManager::Secret leaves the secret in a scheduled-deletion state instead of removing it outright, so DescribeSecret keeps succeeding after DELETE_COMPLETE.

AWS: Smithy — `DescribeSecret` declares `ResourceNotFoundException` (client error, HTTP 400); `DeleteSecret` takes `RecoveryWindowInDays` and `ForceDeleteWithoutRecovery` and returns a `DeletionDate`. The recording shows that the CloudFormation AWS::SecretsManager::Secret handler deletes the secret outright on stack delete (no recovery window is left behind): once the stack reaches DELETE_COMPLETE, `DescribeSecret(SecretId=<the ARN the stack output>)` is refused with code `ResourceNotFoundException` and message `Secrets Manager can't find the specified secret.` — that message is entirely fixed text with no variable part. The consumer-side effect in the same recording is that `GetParameter(Name=/aws/reference/secretsmanager/<secret name>)` then refuses with `ParameterNotFound` and the message `An error occurred (ParameterNotFound) when referencing Secrets Manager: Secret aws/reference/secretsmanager/<secret name> not found. Secrets Manager can't find the specified secret.` — fixed text apart from the reference path, which comes from the request.

Emulator: after DELETE_COMPLETE, `DescribeSecret` answers HTTP 200 and returns the secret with `DeletedDate` set (and `VersionIdsToStages` still populated), i.e. the secret was merely scheduled for deletion. Because the secret still resolves, the Parameter Store reference assertion is never even reached.

Why: a client that tears down a stack to revoke a credential finds the credential still readable — the secret material is still served by DescribeSecret/GetSecretValue and by the `/aws/reference/secretsmanager/` Parameter Store path. That is a confidentiality regression, and it also breaks the common create/delete/recreate loop: the name is still taken, so recreating the stack collides instead of succeeding.

Fix: the AWS::SecretsManager::Secret resource handler's delete path — on stack delete it must delete the secret without a recovery window (the ForceDeleteWithoutRecovery semantics) and the Secrets Manager state must then treat the ARN as absent, so DescribeSecret refuses with ResourceNotFoundException and the SSM `/aws/reference/secretsmanager/` resolver refuses with ParameterNotFound.

Traces: cloudformation__secretsmanager__ssm.cloudformation-stack-deletion-removes-the-secret-and-the-ssm-reference-then-404s.

## Evidence
Expected, after the stack reaches DELETE_COMPLETE:
- `DescribeSecret(SecretId=<stack output SecretArn>)` -> HTTP 400, Code `ResourceNotFoundException`, Message `Secrets Manager can't find the specified secret.`
- `GetParameter(Name=/aws/reference/secretsmanager/<stack name>-secret)` -> Code `ParameterNotFound`.

Actual (check cloudformation-stack-deletion-removes-the-secret-and-the-ssm-reference-then-404s, verdict mismatch):
```
AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {'ARN': 'arn:aws:secretsmanager:...:secret:...-secret-A0VAPt', 'Name': '...-secret', 'RotationEnabled': False, 'DeletedDate': datetime..., 'VersionIdsToStages': {...: ['AWSCURRENT']}, ..., 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
```
The divergence is the 200-with-DeletedDate instead of a 400 ResourceNotFoundException; the ARN suffix, names and timestamps in that payload are per-run values, not requirements.

## Steps to Reproduce
1. CreateStack with a single AWS::SecretsManager::Secret (Name `${AWS::StackName}-secret`, GenerateSecretString) and an Output `SecretArn: !Ref Secret`; wait for CREATE_COMPLETE.
2. Confirm `GetParameter(Name=/aws/reference/secretsmanager/<stack name>-secret, WithDecryption=true)` resolves to a non-empty value.
3. DeleteStack and wait for DELETE_COMPLETE.
4. Call `DescribeSecret(SecretId=<SecretArn>)`: the emulator returns 200 with DeletedDate set; AWS refuses with ResourceNotFoundException / `Secrets Manager can't find the specified secret.`
