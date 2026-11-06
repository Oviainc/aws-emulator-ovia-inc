Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative
Issue-Key: dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative|_invoke_denied#6|Timeout|return json.loads(resp["Payload"].read())
Issue-Commit: 77172d4b913d3cd289b60abc3e2fa4659d3ee8a2

# dynamodb__lambda__s3__secretsmanager: a CloudFormation-deployed Lambda whose role withholds secretsmanager:GetSecretValue never returns the handler's AccessDeniedException verdict, so the permission-boundary negative is unobservable

> _Note: this issue was identified against the code base at commit `77172d4b913d3cd289b60abc3e2fa4659d3ee8a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke, GetSecretValue

## Description
ONE root cause, grounded in 1 failing behavioral check under the suite heading `dynamodb__lambda__s3__secretsmanager`. The check is marked neither ALREADY FILED nor OWNED in the replay report.

Related open issue that does NOT carry this replay key (it owns the sibling lambda-sdk-* and cli-* checks in this same service): 20261105T081139Z-dynamodb-lambda-s3-secretsmanager-the-in-process-lambda.md. Its sibling evidence in this very run shows the in-process Python runtime answering every invoke with `{'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError'}`, which is the most likely mechanism here too, but that issue does not carry this check, so this one exists to get the id below re-replayed. Also related: 20261029T180026Z-dynamodb-lambda-s3-secretsmanager-secrets-manager-rotatesecret-is-accepted-for-an-un.md.

Cause 1 of 1: a CloudFormation stack that provisions a Secret, a Lambda and an execution role whose secretsmanager:GetSecretValue statement is withheld by a template Condition reaches CREATE_COMPLETE, but invoking the function never yields the handler's own verdict payload, so the refusal the missing grant must produce is never observable.
AWS: Smithy (com.amazonaws.lambda#Invoke) returns the handler's payload in the response body; com.amazonaws.secretsmanager#GetSecretValue is refused for a caller without the grant with error code `AccessDeniedException` — the SDK surfaces that code to the handler, which this check's handler catches and reports as `{'ok': False, 'error_code': 'AccessDeniedException'}`. Recorded behavior: invoking the denied-variant function returns that payload, and invoking the identical stack deployed with `AllowSecretsAccess=true` returns `{'ok': True, 'secret': '<the deployed SecretString>'}`. Both legs are the check's oracle; the second is the positive control that proves the refusal is attributable to the withheld statement and not to the plumbing.
Emulator: neither payload ever arrives. The check polls Invoke for a payload whose `ok` is False and the harness budget expires first — `Error: Check timed out after 60.0s`. No AccessDeniedException is produced and no secret value is returned; the handler's result is never surfaced at all.
Why: this is the canonical least-privilege negative test. A client cannot establish that withholding secretsmanager:GetSecretValue actually refuses the read, so a permission boundary validated against this emulator was never exercised in either direction; and because the positive control is equally unreachable, a developer cannot even tell whether the function or the authorization is at fault.
Fix: the Lambda invoke path for a CloudFormation-provisioned function — the in-process runtime must execute the deployed handler and return its payload (including the AWS SDK the handler imports), and the Secrets Manager GetSecretValue call made with the function's execution role must be authorized against that role's policy and refused with `AccessDeniedException` when the grant is absent.
Traces: dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative.

## Evidence
[mismatch] dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative
  Error: Check timed out after 60.0s
  The check deploys one template twice via CreateStack, parameterised by AllowSecretsAccess ('false' then 'true'), where a Conditions block gates the role's secretsmanager:GetSecretValue statement. It then polls `lambda.invoke(FunctionName=<denied fn>, Payload=b'{}')` until the parsed payload has ok is False (timeout_s=90), asserting afterwards that payload['error_code'] == 'AccessDeniedException'; then the same poll for ok is True on the allowed stack, asserting payload['secret'] == 'top-secret-value'.
  Expected: {'ok': False, 'error_code': 'AccessDeniedException'} from the denied function and {'ok': True, 'secret': '<SecretString>'} from the control.
  Actual: the first poll never converges — the harness 60 s cap fires before the check's own 90 s poll. Sibling checks in this same service in this run show every invoke answering {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError'} (dynamodb__lambda__s3__secretsmanager.cli-create-secret-and-invoke-lambda-fetch), i.e. the handler's own result is never produced.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. CreateStack with a template declaring: an AWS::SecretsManager::Secret with SecretString 'top-secret-value'; an AWS::IAM::Role trusted by lambda.amazonaws.com with AWSLambdaBasicExecutionRole; a conditional inline policy granting secretsmanager:GetSecretValue on the secret, gated on Parameters.AllowSecretsAccess == 'true'; and an AWS::Lambda::Function whose handler calls secretsmanager:GetSecretValue and returns {'ok': True, 'secret': <value>} or, on ClientError, {'ok': False, 'error_code': <Error.Code>}. Deploy with AllowSecretsAccess='false' and wait for CREATE_COMPLETE.
3. lambda:Invoke the deployed function with Payload b'{}' and parse the response payload. AWS returns {'ok': False, 'error_code': 'AccessDeniedException'}; the emulator returns no handler verdict (observed sibling behavior: a Runtime.ImportModuleError for boto3).
4. Deploy the identical stack with AllowSecretsAccess='true' and invoke again: AWS returns {'ok': True, 'secret': 'top-secret-value'}.
5. Re-replay dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative.
