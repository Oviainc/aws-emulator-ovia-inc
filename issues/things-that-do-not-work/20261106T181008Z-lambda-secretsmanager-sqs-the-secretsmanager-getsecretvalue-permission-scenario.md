Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied
Issue-Key: lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied|_poll_until#8|Timeout|last = probe()
Issue-Commit: 85bf1a921c90e8ec0578295db955b3f20c9762fc

# lambda__secretsmanager__sqs: the secretsmanager:GetSecretValue permission scenario never settles — the in-handler Secrets Manager read neither succeeds when granted nor is refused AccessDeniedException when the grant is removed, so the check is killed at 60 s

> _Note: this issue was identified against the code base at commit `85bf1a921c90e8ec0578295db955b3f20c9762fc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
Cause 1 of 1: a Lambda handler's Secrets Manager read is not served through the execution role's identity policy, so neither half of the grant/deny scenario is observable and the check never reaches a verdict.

NOTE: the open issue 20261106T134622Z-lambda-secretsmanager-sqs-the-in-lambda-sdk-reports.md lists this check id under its cause 4, but its recorded replay key for this check (_poll_until#8|Timeout|time.sleep(interval_s)) no longer matches the failure this run produced (the harness now kills the check with no in-check frame at all), so the suite reports the check unowned. Whatever now stalls the scenario is therefore not covered by that issue's key; the sibling issue 20261106T153504Z-lambda-secretsmanager-sqs-a-function-whose-role-grants.md describes the same shape of stall for a different check.

AWS: a function invoked with RequestResponse runs its handler under the execution role's credentials. With an identity policy that allows secretsmanager:GetSecretValue on the secret's ARN, the handler's GetSecretValue returns HTTP 200 carrying that version's SecretString (Smithy: secretsmanager GetSecretValue, input SecretId required @length 1..2048; output ARN/Name/VersionId/SecretString(sensitive, @length 1..65536)/VersionStages/CreatedDate; modeled errors DecryptionFailure, InternalServiceError, InvalidParameterException, InvalidRequestException, ResourceNotFoundException — all client errors at HTTP 400 except InternalServiceError). When that statement is replaced by a logs-only policy, the identical call is refused with HTTP 400 and error code AccessDeniedException whose message is AWS's documented implicit-deny text: "User: <assumed-role principal ARN> is not authorized to perform: secretsmanager:GetSecretValue on resource: <secret ARN> because no identity-based policy allows the secretsmanager:GetSecretValue action". Everything outside the angle brackets is fixed text; the principal comes from the invoking role session (arn:aws:sts::<account>:assumed-role/<role name>/<function name>) and the resource ARN from the request. Restoring the statement returns the identical call to 200. The refusal classification and message are documented IAM behavior; the rest is recorded behavior only.

Emulator: the scenario never produces either outcome — the check is killed by the harness with "Error: Check timed out after 60.0s" while polling, so no 200-with-SecretString control result and no AccessDeniedException (neither its HTTP status, nor its code, nor its message) is ever served. Because the control phase never satisfies its predicate, the deny phase and the restore phase are never exercised at all.

Why: a client cannot distinguish a correctly scoped execution role from one with no grant at all. A handler that is correctly permitted gets no usable secret value, and a least-privilege regression (grant removed) produces no refusal, so the blast-radius control Secrets Manager relies on is unverifiable and, as served, silently absent.

Fix: the in-process Lambda runtime that Invoke uses for the handler (its bundled AWS SDK's secretsmanager client and the execution-role credentials/endpoint it hands the handler), together with the identity-policy evaluation applied to secretsmanager:GetSecretValue for an assumed-role principal in the GetSecretValue handler's authorization path — including emission of the AccessDeniedException message template above.

Traces: lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied.

## Evidence
Expected (AWS): phase 1 — Invoke returns 200 and the handler document reports ok=true with secret=='top-secret-value'; phase 2 (role's inline policy replaced by the logs-only baseline) — the handler catches a client error with code AccessDeniedException and message "An error occurred (AccessDeniedException) when calling the GetSecretValue operation: User: arn:aws:sts::<account>:assumed-role/<role name>/<function name> is not authorized to perform: secretsmanager:GetSecretValue on resource: <secret ARN> because no identity-based policy allows the secretsmanager:GetSecretValue action" (fixed text as shown; principal, function name and secret ARN are values taken from the request/IAM state, not literals of any one run); phase 3 (grant restored) — the identical Invoke returns 200 with the secret again.

Actual (emulator): "Error: Check timed out after 60.0s" — the polling loop never observes ok=true for the granted control, and consequently never observes ok=false with an AccessDenied code/message for the removed grant. No refusal status, code or message is served at any point.

## Steps to Reproduce
1. secretsmanager CreateSecret with SecretString 'top-secret-value'.
2. iam CreateRole trusting lambda.amazonaws.com with an inline policy allowing logs:CreateLogGroup/CreateLogStream/PutLogEvents plus secretsmanager:GetSecretValue on that secret's ARN; lambda CreateFunction (python3.12) with SECRET_ARN in the environment and a handler that calls secretsmanager GetSecretValue(SecretId=$SECRET_ARN) and returns {ok, secret} or {ok:false, error, message}.
3. lambda Invoke (RequestResponse) in a loop: ok=true with secret=='top-secret-value' is never returned.
4. iam PutRolePolicy replacing 'inline' with the logs-only statement, then Invoke in a loop: no AccessDeniedException code/message is ever returned.
5. PutRolePolicy restoring the GetSecretValue statement, then Invoke: the call should return 200 with the secret again.
6. Replay check lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied.
