Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-denied-without-create-permission
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected|_invoke_until_logged#17|KeyError|request_id = resp["ResponseMetadata"]["HTTPHeaders"]["x-amzn-requestid"]
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke|_invoke_until_logged#16|KeyError|request_id = resp["ResponseMetadata"]["HTTPHeaders"]["x-amzn-requestid"]
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-denied-without-create-permission|_invoke_until_logged#16|KeyError|request_id = resp["ResponseMetadata"]["HTTPHeaders"]["x-amzn-requestid"]
Issue-Commit: 8f45cbde1fc786a42a670207326215689e25a831

# cloudformation__iam__lambda__logs: lambda Invoke responses carry no x-amzn-RequestId header, so an invocation can never be correlated with its log lines

> _Note: this issue was identified against the code base at commit `8f45cbde1fc786a42a670207326215689e25a831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause.

Cause 1 of 1: the 200 response to `lambda:Invoke` is returned with no `x-amzn-RequestId` header, so the invocation has no request id a client can read.

AWS: every AWS API response, `lambda:Invoke` included, carries the `x-amzn-RequestId` response header. botocore surfaces it as `ResponseMetadata.RequestId` and in `ResponseMetadata.HTTPHeaders["x-amzn-requestid"]`. For Lambda specifically that id IS the invocation's request id: it is the value the runtime prints in the function's `START RequestId: ...` / `END` / `REPORT` lines in `/aws/lambda/<function>`, which is the only way an operator ties one invocation to the log events it produced. Smithy: Invoke's output shape `InvocationResponse` has no `RequestId` member — the id is a protocol-level response header, not a modeled member — so this is the AWS wire convention plus the recorded behavior, not a modeled field.

Emulator: the Invoke response has no `x-amzn-RequestId` header at all; reading `ResponseMetadata.HTTPHeaders["x-amzn-requestid"]` raises `KeyError: 'x-amzn-requestid'`. The requirement is the presence of the header on every Invoke response and that its value is the same id the function's own log lines carry; the particular id value in any one run is not part of the requirement.

Why: a client cannot correlate a call with its logs. Every operator workflow built on "invoke, take the request id, grep the log group for it" breaks — including the three recorded ones here, which use the request id to find the line their invocation wrote in the declared vs. implicit `/aws/lambda/<function>` log group. The absence also silently defeats any retry/idempotency logging keyed on the request id.

Fix: the response-header path of the lambda Invoke handler (and the lambda service's common response serializer, if the header is emitted centrally) must set `x-amzn-RequestId`, using the same id the in-process runtime reports as the invocation's `requestId` and writes into the function's log stream.

Traces: the three failing checks in trace_ids, all of which fault at the same line, `resp["ResponseMetadata"]["HTTPHeaders"]["x-amzn-requestid"]`, immediately after `lambda.invoke(...)`.

## Evidence
Expected (AWS): `invoke` → 200 with header `x-amzn-RequestId: <uuid>`; botocore exposes it at `ResponseMetadata.RequestId` and `ResponseMetadata.HTTPHeaders["x-amzn-requestid"]`, and the same id appears in the `START RequestId:`/`REPORT RequestId:` lines of `/aws/lambda/<function>`.

Actual (emulator), identically in all three checks:
`request_id = resp["ResponseMetadata"]["HTTPHeaders"]["x-amzn-requestid"]` → `KeyError: 'x-amzn-requestid'`
- cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected (`_invoke_until_logged`, line 376)
- cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke (`_invoke_until_logged`, line 248)
- cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-denied-without-create-permission (`_invoke_until_logged`, line 252)

## Steps to Reproduce
1. Deploy a stack declaring an `AWS::IAM::Role` and an `AWS::Lambda::Function` (python3.12).
2. Call `lambda:Invoke` with `InvocationType=RequestResponse`.
3. Read `response["ResponseMetadata"]["HTTPHeaders"]["x-amzn-requestid"]` — it is absent (and `ResponseMetadata.RequestId` is unset).
