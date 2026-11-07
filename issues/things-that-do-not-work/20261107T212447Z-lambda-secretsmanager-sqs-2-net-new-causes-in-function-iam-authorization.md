Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__secretsmanager__sqs.lambda-no-permissions-denied-both-services lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied lambda__secretsmanager__sqs.secret-resource-scoped-denies-other-secret lambda__secretsmanager__sqs.deployment-cfn-sqs-lambda-secret
Issue-Key: lambda__secretsmanager__sqs.lambda-no-permissions-denied-both-services|_invoke_until#17|AssertionError|raise AssertionError(
Issue-Key: lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied|_poll_until#13|AssertionError|raise AssertionError(
Issue-Key: lambda__secretsmanager__sqs.secret-resource-scoped-denies-other-secret|run#13|AssertionError|assert payload["other_ok"] is False, (
Issue-Key: lambda__secretsmanager__sqs.deployment-cfn-sqs-lambda-secret|_poll_until#11|AssertionError|raise AssertionError(
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# 2 net-new causes: in-function IAM authorization not enforced for Secrets Manager and SQS, and a stack's secret ARN does not resolve

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
Two net-new root causes. The open issue 20261106T134622Z-lambda-secretsmanager-sqs-the-in-lambda-sdk-reports.md covers other checks in this service; neither cause below carries its replay key.

Cause 1 of 2 — IAM is not enforced for Secrets Manager or SQS calls made from inside a Lambda: a role with no grant, a revoked grant, or a grant scoped to a different resource all succeed.
AWS: recorded behavior only. GetSecretValue and SendMessage from a function are evaluated against the execution role's identity policy; a role with no statement for the action is refused with AccessDeniedException, removing a previously attached policy makes subsequent calls refused once the change propagates, and a policy whose Resource names secret A (or queue A) does not authorize secret B (or queue B).
Emulator: every leg succeeds. Replay details: `AssertionError: subject (no permissions) is denied on both services not met within 45.0s. Last result: {'status_code': 200, 'payload': {'secret_ok': True, 'secret_value': 'unreachable', 'sqs_ok': True}}`; `AssertionError: the removed grant is enforced (IAM policy-removal propagation) not met within 60s. Last result: {'status_code': 200, 'payload': {'ok': True, 'secret': 'top-secret-value'}}`; `AssertionError: policy scoped to secret A must not grant access to secret B, got {'own_ok': True, 'own_secret': 'secret-a-value', 'other_ok': True, 'other_secret': 'secret-b-value'}`.
Why: the execution role is not a boundary. Any function reads any secret and writes any queue in the account, revoking a policy has no effect, and resource-scoped least privilege is cosmetic — and because the calls return 200 the client has no way to notice.
Fix: authorization of in-function AWS calls — resolve the invocation's execution-role principal and evaluate the identity policy (action and Resource scope) on the Secrets Manager and SQS handlers, refusing with AccessDeniedException.
Traces: lambda__secretsmanager__sqs.lambda-no-permissions-denied-both-services, lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied, lambda__secretsmanager__sqs.secret-resource-scoped-denies-other-secret.

Cause 2 of 2 — a CloudFormation-deployed function cannot read the secret the same stack created.
AWS: recorded behavior only. A stack that declares an AWS::SecretsManager::Secret and passes its Ref/ARN into the function's environment leaves the function able to GetSecretValue that ARN after CREATE_COMPLETE, so the pipeline's effect is observable.
Emulator: the handler's GetSecretValue is refused as not found. Replay detail: `AssertionError: effect not met within 120s. Last result: [... {'message': "[ERROR] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetSecretValue operation: Secrets Manager can't find the specified secret."} ...]` read out of the function's own log group — the ARN handed to the function by the stack does not name a secret the service can find.
Why: a client's stack reports CREATE_COMPLETE and the application is broken at the first request: the secret reference the template resolved is unusable, so no deployment that injects a secret ARN through CloudFormation works.
Fix: the CloudFormation AWS::SecretsManager::Secret resource handler's Ref/ARN resolution (the identifier it publishes must be the one GetSecretValue accepts), and/or the GetSecretValue lookup of that identifier.
Traces: lambda__secretsmanager__sqs.deployment-cfn-sqs-lambda-secret.

## Evidence
Cause 1: expected AccessDeniedException for a role with no grant, for a removed grant, and for a secret/queue outside the policy's Resource; actual {'secret_ok': True, 'sqs_ok': True}, {'ok': True, 'secret': 'top-secret-value'} and {'own_ok': True, 'other_ok': True, 'other_secret': 'secret-b-value'}.
Cause 2: expected the stack-deployed function to read the stack's secret after CREATE_COMPLETE; actual ResourceNotFoundException "Secrets Manager can't find the specified secret." in the function's log.

## Steps to Reproduce
Cause 1: CreateFunction whose handler calls GetSecretValue and SendMessage; give its role no statements for either service and Invoke (both succeed). Then attach and detach a grant and Invoke again (still succeeds). Then scope the policy's Resource to secret A / queue A and have the handler touch B (succeeds).
Cause 2: CreateStack with AWS::SecretsManager::Secret, AWS::SQS::Queue and AWS::Lambda::Function whose Environment carries !Ref/!GetAtt of the secret; after CREATE_COMPLETE Invoke the function and read its log group — GetSecretValue is ResourceNotFoundException.
