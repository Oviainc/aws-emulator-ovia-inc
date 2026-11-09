Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__secretsmanager__ssm.cdk-deploy-generated-secret-and-tagged-parameter-read-back-then-destroyed
Issue-Key: cloudformation__secretsmanager__ssm.cdk-deploy-generated-secret-and-tagged-parameter-read-back-then-destroyed|run#33|AssertionError|assert (secret_code, secret_message) == (
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# `cdk destroy` of a CDK-generated SecretsManager secret only schedules it for deletion — GetSecretValue answers InvalidRequestException instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSecret

## Description
Cause 1 of 1: deleting the stack that owns an AWS::SecretsManager::Secret leaves the secret merely marked for deletion instead of removing it, so reads of the deleted secret are refused with the marked-for-deletion error rather than not-found.

AWS: when CloudFormation deletes an AWS::SecretsManager::Secret (the CDK default removal policy for a generated secret), the secret is removed outright — no recovery window is applied for the stack-deletion path — and GetSecretValue on its ARN answers 400 ResourceNotFoundException with message `Secrets Manager can't find the specified secret.` Smithy (secretsmanager): GetSecretValue errors include ResourceNotFoundException and InvalidRequestException; InvalidRequestException is the error for a secret that is scheduled for deletion, and its message is `You can't perform this operation on the secret because it was marked for deletion.` AWS returns the first, not the second, once the stack has been destroyed.

Emulator: after `cdk destroy`, GetSecretValue on the secret's ARN answers `InvalidRequestException`: `You can't perform this operation on the secret because it was marked for deletion.` — i.e. the stack delete only scheduled the secret, leaving it in the recovery window.

Why: a client that tears an estate down and re-deploys it hits a name/ARN that is still occupied by a scheduled-for-deletion secret: the re-create is refused, and cleanup verification that expects ResourceNotFoundException instead sees a different error class and concludes the teardown failed. The two errors also mean opposite things operationally — "gone" versus "still there, recoverable".

Fix: the AWS::SecretsManager::Secret resource handler's DELETE path — a stack delete must perform the immediate deletion (no recovery window) so the secret is absent, rather than issuing a scheduled DeleteSecret.

Traces: check cloudformation__secretsmanager__ssm.cdk-deploy-generated-secret-and-tagged-parameter-read-back-then-destroyed.

Related but not covering: open issues 20261108T215449Z-secretsmanager-2-net-new-causes-a-stack-created.md and 20261109T043550Z-secretsmanager-5-net-new-causes-cloudformation-stack-delete.md describe similar scheduling behaviour under the `secretsmanager` heading; this failing check is listed under cloudformation__secretsmanager__ssm and carries its own replay key.

## Evidence
Expected: after `cdk destroy`, secretsmanager GetSecretValue(SecretId=<secret ARN>) raises ResourceNotFoundException (message `Secrets Manager can't find the specified secret.` — fixed text, no variable parts).

Actual: `AssertionError: after cdk destroy, GetSecretValue('arn:aws:secretsmanager:<region>:<account>:secret:<stack>-SecretA720EF05-<suffix>') returned 'InvalidRequestException': "You can't perform this operation on the secret because it was marked for deletion.", expected an outright ResourceNotFoundException`.
The ARN in the message is a per-run generated value; the requirement is the error code/message class: ResourceNotFoundException, not InvalidRequestException.

## Steps to Reproduce
1. `cdk deploy` a stack with a generated secretsmanager Secret (default removal policy) and a tagged SSM StringParameter; read both back.
2. `cdk destroy` the stack and wait for DELETE_COMPLETE.
3. secretsmanager GetSecretValue(SecretId=<the secret's ARN>) — it raises InvalidRequestException "...marked for deletion." instead of ResourceNotFoundException.
