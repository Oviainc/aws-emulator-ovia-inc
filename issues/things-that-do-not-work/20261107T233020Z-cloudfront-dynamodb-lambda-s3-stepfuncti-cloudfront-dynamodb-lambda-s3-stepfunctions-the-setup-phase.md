Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__dynamodb__lambda__s3__stepfunctions.e2e-cloudfront-distribution-origin-error-diagnosis-lifecycle
Issue-Key: cloudfront__dynamodb__lambda__s3__stepfunctions.e2e-cloudfront-distribution-origin-error-diagnosis-lifecycle|phase#10|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 69a812023dc7b19082ccb88a67a077b5f3204d48

# cloudfront__dynamodb__lambda__s3__stepfunctions: the setup phase's workflow fails with UnknownServiceError — the Lambda runtime's bundled botocore knows no 's3' client

> _Note: this issue was identified against the code base at commit `69a812023dc7b19082ccb88a67a077b5f3204d48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause, 1 failing check.

Cause 1 of 1: the lifecycle's setup phase runs a provisioning workflow whose Lambda handler calls boto3.client("s3"); the handler dies with UnknownServiceError, so the execution ends FAILED and the phase aborts before the distribution-diagnosis subject is reached.
  AWS: recorded behavior only for the runtime's SDK inventory. The recording shows the handler's s3.put_object(...) succeeding and the setup workflow for each tenant ending SUCCEEDED, after which the check proceeds to its origin-error diagnosis phases.
  Emulator: setup's workflow ends 'FAILED' with error 'UnknownServiceError' and the body {"errorMessage":"Unknown service: 's3'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager, sesv2", "errorType":"UnknownServiceError", ...} raised by the runtime's own botocore shim at /var/task/botocore/client.py (_raise_refusal). The bundled catalog is a fixed nine-service list omitting s3, and that valid-service sentence is emulator-authored.
  Why: a function cannot use S3 at all, so any CloudFront-origin scenario that stages content through Lambda is unrunnable; the client sees an error class AWS's runtime never raises for a real service name.
  Note: the same cause is filed under the dynamodb__lambda__s3__stepfunctions heading for its three checks; this issue carries this service's check. Open issue 20261106T213332Z-cloudfront-dynamodb-lambda-s3-stepfuncti-cloudfront-dynamodb-lambda-s3-stepfunctions-the-state-machine.md describes an earlier failure point of this check (a 2-second interpretation budget) and not this one.
  Fix: the Lambda runtime's bundled SDK/service-model inventory in the lambda service crate (services/service-com-amazonaws-lambda-awsgirapiservice, the in-process python runtime's botocore data catalog and endpoint resolution) must include s3.
  Traces: cloudfront__dynamodb__lambda__s3__stepfunctions.e2e-cloudfront-distribution-origin-error-diagnosis-lifecycle.

## Evidence
AssertionError: [phase 2/5 setup] setup's workflow for basalt-tenant ended 'FAILED': 'UnknownServiceError' '{"errorMessage":"Unknown service: \'s3\'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager, sesv2","errorType":"UnknownServiceError","requestId":"a2119290-...","stackTrace":["  File \"/var/task/index.py\", line 41, in ..."]}'

Expected: the setup workflow SUCCEEDED. Tenant name and request id are per-run values; the requirement is that boto3.client("s3") constructs inside the runtime.

## Steps to Reproduce
1. Create a python Lambda whose handler constructs boto3.client("s3") and calls put_object.
2. Invoke it (directly or through a Step Functions Task) — the emulator's runtime raises UnknownServiceError naming 's3' as unknown; AWS's runtime builds the client and the call succeeds.
