Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role
Issue-Key: kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role|_poll_until#13|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# The in-process Lambda runtime's bundled SDK has no s3 client, so an SSE-KMS key-policy denial cannot be exercised

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause, grounded in a failing check this report does NOT mark ALREADY FILED or OWNED. Open issue 20261029T175759Z-kms-lambda-s3-headobject-reports-ssekmskeyid-as-a.md owns another check of this service and does not carry this replay key.

Cause 1 of 1: function code cannot construct an S3 client, so no SSE-KMS read is ever attempted.
AWS: the managed python runtime's boto3 builds a client for any service. A role granted s3:GetObject and kms:Decrypt reads an SSE-KMS object (200 with the plaintext); after the key policy denies that role, the same read is refused 403 AccessDenied naming the KMS denial, while a control object under a different key stays readable.
Emulator: the handler faults before either read. The check polls 120 s for the two baseline reads and reports "both SSE-KMS objects readable by the Lambda role before the key-policy change not met within 120.0s. Last result: {'denied.txt': {'code': 'UnknownServiceError'}, 'control.txt': {'code': 'UnknownServiceError'}}" — the bundled SDK's client factory refuses 's3' (its registry lists only sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager).
Why: the baseline both-readable control cannot pass, so the key-policy denial it gates is unfalsifiable; a client cannot test KMS-backed access control from function code at all.
Fix: the in-process Lambda runtime's bundled AWS SDK shim — its service registry and the client factory that refuses unknown services.
Traces: kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role.

## Evidence
Expected both SSE-KMS objects readable by the role, then the denied one refused 403 AccessDenied after the key policy change. Actual: {'denied.txt': {'code': 'UnknownServiceError'}, 'control.txt': {'code': 'UnknownServiceError'}} for 120s — the client for 's3' could not be built inside the runtime.

## Steps to Reproduce
1) Create two CMKs and write one SSE-KMS object under each; grant the function's role s3:GetObject and kms:Decrypt on both.
2) Invoke a handler that calls boto3.client('s3').get_object on both keys.
3) Observe UnknownServiceError instead of the two object bodies.
