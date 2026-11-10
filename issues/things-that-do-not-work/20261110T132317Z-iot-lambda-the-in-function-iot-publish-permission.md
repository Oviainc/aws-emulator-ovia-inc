Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__lambda.lambda-sdk-iot-publish-missing-permission-negative
Issue-Key: iot__lambda.lambda-sdk-iot-publish-missing-permission-negative|_invoke#13|Timeout|return body
Issue-Commit: b65ac132d2656b2cf62c7f436b7f396df22480d4

# iot__lambda: the in-function iot:Publish permission-boundary negative reaches no verdict — the Lambda->IoT invoke trajectory hangs to the 60 s harness cap instead of returning a Payload

> _Note: this issue was identified against the code base at commit `b65ac132d2656b2cf62c7f436b7f396df22480d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause, grounded in the single net-new failing iot__lambda check of this run (the other 31 iot__lambda failures are marked OWNED/ALREADY FILED and are not part of this issue).

Cause 1 of 1: the Lambda->IoT invoke trajectory that this permission-boundary negative drives never settles — the check reaches NO verdict at all and is cut off by the harness at 60 s, so neither the positive control nor the IAM-denial decision under test is ever observed. Failing check: iot__lambda.lambda-sdk-iot-publish-missing-permission-negative.

Relation to an already-open issue: 20261105T114938Z-iot-lambda-the-python-lambda-runtime-provides-no.md (open) lists this same check id, but it is grounded in a DIFFERENT observed behaviour — a prompt, in-band refusal: Invoke answering HTTP 200 with X-Amz-Function-Error=Unhandled and a {"errorType": "Runtime.ImportModuleError"} Payload, recorded there for this check as an AssertionError raised out of the suite's `_invoke` helper. The replay report did not mark this check as owned, and what it now does is not that: there is no FunctionError document and no assertion, only a 60 s hang. A bare runtime that cannot import boto3 fails at module init in milliseconds; it cannot produce a stall. So the stall is a separate defect on the same path and is filed here rather than folded into that issue. Fixing the runtime-provided SDK alone will not close this check.

AWS: Smithy (lambda): Invoke, POST /2015-03-31/functions/{FunctionName}/invocations, http code 200, output InvocationResponse{StatusCode (@httpResponseCode), FunctionError (header X-Amz-Function-Error), LogResult (X-Amz-Log-Result), Payload (@httpPayload), ExecutedVersion (X-Amz-Executed-Version)}. A RequestResponse Invoke is synchronous and bounded by the function's configured Timeout — here CreateFunction sets Timeout=30 — so it ALWAYS terminates: either with the handler's own Payload, or, when the handler exceeds its Timeout, with HTTP 200 + FunctionError and a {"errorMessage": "<n> ... Task timed out after <Timeout>.<frac> seconds", "errorType": "Sandbox.Timedout"}-class document. Smithy's modeled Invoke errors (InvalidRequestContentException, ResourceNotFoundException, TooManyRequestsException, ServiceException, ResourceNotReadyException, ...) are all answered promptly too; none of them is an open connection. On the IoT side, Smithy (iot-data): Publish, POST /topics/{topic}, http code 200, errors InternalFailureException(500), InvalidRequestException(400), MethodNotAllowedException(405), ThrottlingException(429), UnauthorizedException(401); the IAM-denial code this check asserts, ForbiddenException with HTTP 403, is recorded behavior only — the HTTPS data-plane endpoint refuses a SigV4-signed publish whose caller identity has no iot:Publish grant on arn:aws:iot:<region>:<account>:topic/<topic> with 403 ForbiddenException, and botocore surfaces Error.Code = "ForbiddenException". So on AWS this trajectory completes well inside the budget: the allowed function's Invoke returns {"ok": true}, the IoT rule forwards the published message to the rule's SQS queue, and the denied function's Invoke returns {"ok": false, "error": "ForbiddenException"} and nothing is forwarded.

Emulator: the check produces no response and no refusal — the suite reports exactly `Error: Check timed out after 60.0s` with no exception frame, meaning the hang happened inside a call that never returned rather than at an assertion. Every step on the path has its own bounded poll (CreateFunction role-propagation retry 60 s, State=Active 60 s, `_invoke_until_delivered` 60 s, SQS forwarding poll 90 s), so a step that returned ANY answer — including a FunctionError document, a 403, or a 5xx — would have produced an assertion message naming it. None appears. The emulator therefore leaves one of the operations on this trajectory open past the harness cap: the RequestResponse lambda Invoke of the python3.12 function (Timeout=30) is the operation that must not outlive its own configured timeout, and it is the only call on the path that AWS bounds by a function-level deadline.

Why: a client calling Invoke synchronously relies on the call being bounded — by the handler's result or by the function's Timeout. Against this emulator the call does not come back at all, so an SDK caller blocks until its own socket/read timeout and retries, and the response it would branch on never exists. Concretely, the permission-boundary contract this check exists to prove is unreachable in both directions: the positive control (a role WITH iot:Publish on the topic ARN publishes and the IoT topic rule forwards the payload to SQS) is never confirmed, and the negative (a role WITHOUT iot:Publish gets 403 ForbiddenException from iot-data Publish and nothing leaks to the queue) is never exercised. A hang is strictly worse than a wrong answer here: a wrong error code at least lets a client decide, whereas an unbounded Invoke makes the deny decision indistinguishable from a slow one, so IAM-denial behaviour on the Lambda->IoT data path cannot be tested at all.

Fix: the lambda crate's Invoke handler and the execution component behind it — the RequestResponse invoke path must enforce the function's configured Timeout as a hard deadline on handler execution and always return within it, answering HTTP 200 with the handler's Payload on completion and HTTP 200 + X-Amz-Function-Error plus the timeout error document when the deadline expires, never holding the request open. The in-function iot / iot-data client the handler builds must likewise be pointed at an address the emulator serves and given a bounded connect/read deadline, so an unreachable or unanswered in-function AWS call surfaces as the handler's own caught error inside the invoke's deadline instead of stalling the invoke.

Traces: iot__lambda.lambda-sdk-iot-publish-missing-permission-negative (Error: Check timed out after 60.0s).

## Evidence
Expected (AWS), every call bounded:
- lambda Invoke (InvocationType=RequestResponse) of the python3.12 function with Timeout=30 -> HTTP 200, no X-Amz-Function-Error, Payload = {"ok": true} for the role holding iot:Publish on arn:aws:iot:<region>:<account>:topic/<topic>.
- The IoT topic rule over SELECT * FROM '<topic>' forwards the published JSON to the rule's SQS queue, so receive_message returns the published document.
- lambda Invoke of the sibling function whose role has only iot:DescribeEndpoint -> HTTP 200, no X-Amz-Function-Error, Payload = {"ok": false, "error": "ForbiddenException", "message": "..."}, because iot-data Publish answered HTTP 403 with error code ForbiddenException. Nothing is forwarded to the queue for that publish.
- The whole trajectory completes inside the suite's budget; every Invoke returns no later than the function's configured Timeout.

Actual (emulator), as reported by the suite:
- iot__lambda.lambda-sdk-iot-publish-missing-permission-negative: `Error: Check timed out after 60.0s` — no HTTP status, no error code, no Payload and no assertion frame. The check never reaches its first assertion (`assert body_allowed == {"ok": True}`), so no response was obtained for even the positive control.

Why a hang and not a refusal: every step of this check is individually bounded and asserts with a message on failure — `_poll_until(..., timeout_s=60, description="function <name> create (role propagation)")`, `_poll_until(..., description="function <name> Active")`, `_invoke` (raises AssertionError(f"lambda invocation failed ({FunctionError}): {body!r}") the instant a FunctionError is set), `_invoke_until_delivered(..., timeout_s=60.0)`, and `_poll_until(..., timeout_s=90.0, description="positive-control publish forwarded to SQS via IoT rule")`. Any answered call — a FunctionError document, a 403, a 4xx/5xx — would have surfaced as one of those messages. The bare `Check timed out after 60.0s` means a call was still open when the harness cut the check off.

Message requirement (for the refusal that must eventually be produced): the denied in-function publish must come back as HTTP 403 with error code `ForbiddenException` from iot-data Publish, and the handler's document is then {"ok": false, "error": "ForbiddenException", ...}; `ForbiddenException` is fixed text, while the topic, thing, function and role names, the account id and the region are values taken from the request and from emulator state. If instead the handler exceeds its Timeout, Invoke must answer HTTP 200 with X-Amz-Function-Error set and an errorType of the runtime's timeout class, whose message carries the function's CONFIGURED Timeout (30 here) as its variable part. Generated function/role/queue/topic names, the account id, request ids and wall-clock latencies in any one run are per-run values and are not part of the requirement.

Distinguishing this from the open issue 20261105T114938Z-iot-lambda-the-python-lambda-runtime-provides-no.md: that issue's evidence for this same check is an AssertionError out of `_invoke` carrying HTTP 200 + X-Amz-Function-Error=Unhandled + {"errorMessage": "Unable to import module 'index': No module named 'boto3'", "errorType": "Runtime.ImportModuleError"} — an immediate, in-band answer. This run shows no such answer and no assertion at all, which a module-import failure cannot produce.

## Steps to Reproduce
1. sqs:CreateQueue a queue and read its QueueArn via sqs:GetQueueAttributes.
2. iam:CreateRole a role trusting iot.amazonaws.com and put an inline policy allowing sqs:SendMessage on that queue ARN.
3. iot:CreateTopicRule with sql="SELECT * FROM '<topic>'", awsIotSqlVersion="2016-03-23", ruleDisabled=false and a single sqs action {roleArn: <rule role>, queueUrl: <queue url>, useBase64: false}.
4. iam:CreateRole two roles trusting lambda.amazonaws.com, both with AWSLambdaBasicExecutionRole attached. On the first put an inline policy allowing ONLY iot:DescribeEndpoint on *; on the second allow iot:DescribeEndpoint on * AND iot:Publish on arn:aws:iot:<region>:<account>:topic/<topic>.
5. lambda:CreateFunction two functions, Runtime="python3.12", Handler="index.handler", Timeout=30, Code.ZipFile = a zip containing only index.py whose handler does:
     iot = boto3.client('iot', region_name=event['region'])
     ep = iot.describe_endpoint(endpointType='iot:Data-ATS')['endpointAddress']
     data = boto3.client('iot-data', region_name=event['region'], endpoint_url='https://' + ep)
     data.publish(topic=event['topic'], qos=0, payload=event['payload'].encode('utf-8'))
     return {'ok': True}
   with `except ClientError as e: return {'ok': False, 'error': e.response['Error']['Code'], ...}` and a catch-all returning the same shape. Wait for each to report State=Active.
6. lambda:Invoke the ALLOWED function (InvocationType=RequestResponse) with {"region": <region>, "topic": <topic>, "payload": "{\"marker\": \"<marker>\"}"}. Time the call. On AWS it returns within the function's 30 s Timeout with Payload {"ok": true}; against the emulator the call does not come back and the check is cut off at the 60 s harness cap having produced no response.
7. Poll sqs:ReceiveMessage on the queue for the forwarded marker document (AWS: it arrives).
8. lambda:Invoke the DENIED function with the same shape of event. On AWS it returns within its Timeout with {"ok": false, "error": "ForbiddenException"}, and the marker never appears on the queue across repeated ReceiveMessage samples.
9. Replay the behavioral suite and run iot__lambda.lambda-sdk-iot-publish-missing-permission-negative — it reports `Check timed out after 60.0s` with no status, error code or Payload, instead of either verdict.
