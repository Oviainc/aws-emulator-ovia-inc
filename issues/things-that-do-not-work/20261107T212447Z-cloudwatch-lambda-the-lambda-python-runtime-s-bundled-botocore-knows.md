Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-delivers-alarm-payload-to-function cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-without-invoke-permission-denied
Issue-Key: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-delivers-alarm-payload-to-function|_warm_up#8|AssertionError|raise AssertionError(f"warm-up invoke of {fn_name} kept failing: {body[:500]!r}")
Issue-Key: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-without-invoke-permission-denied|_warm_up#8|AssertionError|raise AssertionError(f"warm-up invoke of {fn_name} kept failing: {body[:500]!r}")
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# The Lambda Python runtime's bundled botocore knows no 'cloudwatch' client, so alarm-action checks cannot run

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: `boto3.client("cloudwatch")` inside a managed Python Lambda raises UnknownServiceError, so a handler cannot call PutMetricData and the CloudWatch alarm-action legs can never be reached.

AWS: recorded behavior only. The managed python3.x runtimes ship a complete boto3/botocore, so `boto3.client("cloudwatch").put_metric_data(...)` resolves the service model and either succeeds or is refused by IAM with AccessDeniedException. Both recordings warm the function up with exactly that call before driving the alarm.

Emulator: the in-function SDK refuses to build the client. Replay detail: `AssertionError: warm-up invoke of ... kept failing: {"errorMessage":"Unknown service: 'cloudwatch'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager","errorType":"UnknownServiceError",...}` raised from `botocore/client.py` `_raise_refusal`. The bundled client only knows eight services; `cloudwatch` is not among them.

Why: any handler that publishes a custom metric or reads CloudWatch from inside Lambda fails with an SDK-construction error instead of a service response. A client cannot tell a missing IAM grant (AccessDeniedException) from an SDK that has no such service, so both the positive path and every negative authorization test are unreachable.

Fix: the in-process Lambda Python runtime's bundled botocore service catalogue — the `cloudwatch` (monitoring) service model must be resolvable so calls reach the emulator's CloudWatch handlers and are authorized there.

Traces: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-delivers-alarm-payload-to-function, cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-without-invoke-permission-denied.

## Evidence
Expected: `boto3.client("cloudwatch").put_metric_data(...)` inside a python3.12 function either succeeds or is refused with AccessDeniedException.
Actual: errorType `UnknownServiceError`, errorMessage "Unknown service: 'cloudwatch'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager".

## Steps to Reproduce
1. CreateFunction (python3.12) whose handler calls boto3.client("cloudwatch").put_metric_data(Namespace=..., MetricData=[...]).
2. Invoke it.
3. The response carries FunctionError and payload errorType=UnknownServiceError naming 'cloudwatch' as unknown.
