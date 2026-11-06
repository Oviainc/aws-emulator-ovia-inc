Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__secretsmanager__sqs.secret-resource-scoped-denies-other-secret
Issue-Key: lambda__secretsmanager__sqs.secret-resource-scoped-denies-other-secret|_invoke_lambda#10|Timeout|resp = ctx.client("lambda").invoke(
Issue-Commit: 81fe993c0fc493a00f42b69c764d54e4324ad80b

# lambda__secretsmanager__sqs: a function whose role grants secretsmanager:GetSecretValue on its own secret never succeeds in reading it, so the resource-scoped deny check never reaches its verdict

> _Note: this issue was identified against the code base at commit `81fe993c0fc493a00f42b69c764d54e4324ad80b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the positive control of resource-scoped authorization never passes — a Lambda whose execution role holds secretsmanager:GetSecretValue on exactly one secret cannot read that secret from inside the handler, so the run exhausts the harness budget before the deny half can be observed.

AWS: a function invoked with RequestResponse runs its handler with the execution role's credentials against the regional endpoint (Smithy: lambda Invoke POST /2015-03-31/functions/{FunctionName}/invocations, StatusCode via httpResponseCode, FunctionError via the X-Amz-Function-Error header, Payload via httpPayload; secretsmanager GetSecretValue returns SecretString). With an identity policy allowing secretsmanager:GetSecretValue on secret A's ARN only, the handler's GetSecretValue on A returns A's SecretString (Invoke answers 200 with no X-Amz-Function-Error and the handler's own document), while the same call on secret B is refused AccessDeniedException with the message 'User: <assumed-role principal arn> is not authorized to perform: secretsmanager:GetSecretValue on resource: <secret B arn> because no identity-based policy allows the secretsmanager:GetSecretValue action' — fixed text with the principal taken from the invoking role session and the resource ARN taken from the request. That message wording is documented IAM behavior; the rest is recorded behavior only.

Emulator: repeated invokes of the control never report success on the function's own secret, so the check never gets past the control poll and ends 'Check timed out after 60.0s'. Because the control never passes, the emulator's resource-scoped deny (and its AccessDeniedException message) is never exercised at all.

Why: resource-scoped IAM for Secrets Manager is unverifiable and, as served, a correctly-permitted function cannot read its own secret — any client whose handler reads a secret gets no usable result, and a deny cannot be distinguished from the control failing.

Fix: the in-process Lambda runtime used by Invoke (the handler's AWS SDK environment and the execution-role credentials/endpoint it is given) together with the identity-policy evaluation applied to secretsmanager:GetSecretValue for that principal.

Traces: lambda__secretsmanager__sqs.secret-resource-scoped-denies-other-secret. The open issue 20261105T081139Z-lambda-secretsmanager-sqs-the-in-process-lambda-python.md lists this check id for the 'no boto3 in the python runtime' cause (Runtime.ImportModuleError), but its replay key no longer matches this failure, so the suite reports it unowned; whatever now stalls the control is not the import failure that issue describes.

## Evidence
Expected (AWS): Invoke returns 200 with the handler's document reporting own_ok=true and own_secret=='secret-a-value' on the first or second attempt, then other_ok=false with code AccessDeniedException and message 'User: arn:aws:sts::<account>:assumed-role/<role name>/<function name> is not authorized to perform: secretsmanager:GetSecretValue on resource: <secret B arn> because no identity-based policy allows the secretsmanager:GetSecretValue action'.
Actual (emulator): the control poll ('policy scoped to secret A grants GetSecretValue on secret A itself', 5 s interval) never becomes true and the check ends 'Check timed out after 60.0s'.

## Steps to Reproduce
1. secretsmanager CreateSecret twice (secret A, secret B).
2. lambda CreateFunction (python runtime) whose role has an inline policy allowing secretsmanager:GetSecretValue on secret A's ARN only, with OWN_SECRET_ARN / OTHER_SECRET_ARN in the environment; the handler calls GetSecretValue on each and returns {own_ok, own_secret, own_code, own_message, other_ok, other_code, other_message}.
3. lambda Invoke (RequestResponse) repeatedly: own_ok never becomes true, so the deny assertion on secret B is never reached.
Replay check lambda__secretsmanager__sqs.secret-resource-scoped-denies-other-secret.
