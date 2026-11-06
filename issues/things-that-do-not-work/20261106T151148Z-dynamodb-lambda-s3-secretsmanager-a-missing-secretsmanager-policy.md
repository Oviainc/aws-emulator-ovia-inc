Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative
Issue-Key: dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative|run#8|AssertionError|assert denied_payload["error_code"] == "AccessDeniedException", (
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# dynamodb__lambda__s3__secretsmanager: a missing secretsmanager policy yields UnknownServiceError instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
One root cause behind 1 failing check.

Cause 1 of 1: the in-process Lambda SDK has no Secrets Manager service model, so a CloudFormation-deployed handler whose role lacks the grant never gets the AWS refusal.
AWS: a handler calling GetSecretValue with an execution role that does not grant `secretsmanager:GetSecretValue` is refused with HTTP 400 and error code `AccessDeniedException`, message `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: secretsmanager:GetSecretValue on resource: <secret-arn> because no identity-based policy allows the secretsmanager:GetSecretValue action` (fixed text; the account, role, function and secret ARN come from the request and the role). Model: Secrets Manager is present in this repo (services/service-com-amazonaws-secretsmanager-secretsmanager), so the client is constructible on the wire.
Emulator: the check sees error code `UnknownServiceError` — the bundled botocore data catalog in the in-process runtime carries only `sqs, events, eventbridge, dynamodb`, so `boto3.client('secretsmanager')` fails before any request is sent and no authorization decision happens.
Why: the negative check cannot distinguish a real IAM denial from a client that could not be built, so a stack deployed with an insufficient role looks "denied" for the wrong reason; and no function can read a secret at all, which breaks the single most common Lambda-secret pattern.
Fix: the in-process Lambda Python runtime's bundled AWS SDK service-model catalog / client factory, plus the IAM authorization path that must answer AccessDeniedException. The open issue 20261105T081139Z-dynamodb-lambda-s3-secretsmanager-the-in-process-lambda.md names the absent SDK for other checks of this service but does not carry this replay key.
Traces: dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative.

## Evidence
Expected (AWS): HTTP 400, Error.Code `AccessDeniedException`, message `User: arn:aws:sts::<account>:assumed-role/<role>/<function> is not authorized to perform: secretsmanager:GetSecretValue on resource: <secret-arn> because no identity-based policy allows the secretsmanager:GetSecretValue action`.

Actual (emulator): AssertionError: expected AccessDeniedException, got 'UnknownServiceError' (handler payload: `Unknown service: 'secretsmanager'. Valid service names are: sqs, events, eventbridge, dynamodb`).

## Steps to Reproduce
1. Deploy a CFN stack with a secret and a Lambda whose execution-role policy omits secretsmanager:GetSecretValue.
2. Invoke the function; its handler calls boto3.client('secretsmanager').get_secret_value(SecretId=...).
3. Expected: AccessDeniedException. Observed: UnknownServiceError from client construction.
