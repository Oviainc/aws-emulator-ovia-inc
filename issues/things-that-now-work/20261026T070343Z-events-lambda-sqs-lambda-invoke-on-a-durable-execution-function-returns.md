Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.xsvc-lambda-1-durable-stop-emits-stopped-status-change-to-sqs
Issue-Key: events__lambda__sqs.xsvc-lambda-1-durable-stop-emits-stopped-status-change-to-sqs|_start#9|AssertionError|assert arn, f"async durable Invoke returned no DurableExecutionArn: {sorted(resp)!r}"
Issue-Commit: 6de2108b200ed90d33fb4f24d0237ce550e05e72

# Lambda Invoke on a durable-execution function returns no DurableExecutionArn, so no durable execution can ever be addressed

> _Note: this issue was identified against the code base at commit `6de2108b200ed90d33fb4f24d0237ce550e05e72`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause.

Cause 1 of 1: Invoke against a function that carries a DurableConfig does not return the DurableExecutionArn of the execution it started, so the caller has no handle for any durable-execution follow-on call (GetDurableExecution, StopDurableExecution, the status-change events keyed on the execution).

AWS: Invoke's output shape InvocationResponse declares a DurableExecutionArn member bound to the response header `X-Amz-Durable-Execution-Arn` (Smithy: Invoke, output_shape InvocationResponse, member DurableExecutionArn @httpHeader("X-Amz-Durable-Execution-Arn"), @length min 1 max 1024, pattern `^arn:([a-zA-Z0-9-]+):lambda:([a-zA-Z0-9-]+):(\\d{12}):function:([a-zA-Z0-9_-]+):(\\$LATEST(?:\\.PUBLISHED)?|[0-9]+)/durable-execution/([a-zA-Z0-9_-]+)/([a-zA-Z0-9_-]+)$`). The request side models the matching input member DurableExecutionName @httpHeader("X-Amz-Durable-Execution-Name"), and Invoke models DurableExecutionAlreadyStartedException (409) — i.e. the operation is modelled as the entry point that STARTS a named durable execution. When Invoke starts one, AWS populates that header, and its value is the ARN the durable-execution APIs and the Lambda status-change events then use. Hypothesis (not stated by the model): the emulator has no durable-execution runtime at all, which is why no ARN is minted.

Emulator: `Invoke(FunctionName=<durable fn>, InvocationType=Event, DurableExecutionName=<name>, Payload=...)` returns a response whose keys are exactly ['Payload', 'ResponseMetadata', 'StatusCode'] — the `X-Amz-Durable-Execution-Arn` header is absent, so botocore surfaces no DurableExecutionArn. The check asserts on it directly: `AssertionError: async durable Invoke returned no DurableExecutionArn: ['Payload', 'ResponseMetadata', 'StatusCode']`.

Why: a client using durable executions gets a 202 that looks like a success but hands back nothing it can address. It cannot call GetDurableExecution or StopDurableExecution, cannot correlate the Lambda status-change events EventBridge delivers (RUNNING/STOPPED/SUCCEEDED carry the execution ARN), and cannot distinguish a started execution from a dropped one. Every durable-execution workflow — including operator-initiated stop, which is what this check grades end to end through an EventBridge default-bus rule to SQS — is unreachable.

Fix: the Invoke handler and its response serializer in the lambda service — Invoke must mint and register a durable execution when the target function has a DurableConfig (and/or when DurableExecutionName is supplied), and emit its ARN in the `X-Amz-Durable-Execution-Arn` response header in the modelled ARN form `arn:aws:lambda:<region>:<account>:function:<name>:<qualifier>/durable-execution/<DurableExecutionName>/<execution-id>`. The registered execution must then be visible to the durable-execution read APIs and be the subject of the emitted status-change events. Related open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md covers Invoke refusing ordinary invocations; it does not cover the missing durable-execution handle on an Invoke that does answer.

Traces: events__lambda__sqs.xsvc-lambda-1-durable-stop-emits-stopped-status-change-to-sqs.

## Evidence
Expected (real AWS): Invoke on a function with a DurableConfig, called with InvocationType=Event and header X-Amz-Durable-Execution-Name=<name>, answers 202 and sets response header `X-Amz-Durable-Execution-Arn` to the started execution's ARN, matching the modelled pattern `arn:<partition>:lambda:<region>:<account-id>:function:<function-name>:<$LATEST|version>/durable-execution/<execution-name>/<execution-id>`. botocore therefore surfaces a `DurableExecutionArn` key in the Invoke response.

Actual (emulator): the Invoke response contains only ['Payload', 'ResponseMetadata', 'StatusCode'] — no DurableExecutionArn, i.e. the `X-Amz-Durable-Execution-Arn` header was not emitted. Failure as reported by the replay:
  check_events__lambda__sqs_xsvc_lambda_1_durable_stop_emits_stopped_status_change_to_sqs.py, line 221, in _start
  AssertionError: async durable Invoke returned no DurableExecutionArn: ['Payload', 'ResponseMetadata', 'StatusCode']

The header name, the member name and the ARN shape above are the requirement (from the Smithy model); the region, account id, function name and execution id in any one run are values filled in from the request and the emulator's state, not part of the template.

## Steps to Reproduce
1. Create an IAM role Lambda can assume and a Python function whose handler uses the durable-execution SDK (decorated with @durable_execution and calling context.wait(...)), configured with a DurableConfig.
2. Wait for the function to reach State=Active.
3. Call lambda:Invoke(FunctionName=<fn>, InvocationType='Event', DurableExecutionName='<unique-name>', Payload=b'{"wait_seconds": 5}').
4. Inspect the response: observe the keys are ['Payload', 'ResponseMetadata', 'StatusCode'] and that no `X-Amz-Durable-Execution-Arn` header was returned, so there is no DurableExecutionArn to pass to StopDurableExecution/GetDurableExecution.
