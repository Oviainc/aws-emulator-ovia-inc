Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__lambda.lambda-sdk-iot-publish-positive
Issue-Key: iot__lambda.lambda-sdk-iot-publish-positive|_invoke_until_delivered#11|Timeout|last_body, last_error = _invoke(ctx, fn_name, payload)
Issue-Commit: 323c298260f641866db820e53cda367b812962dd

# iot__lambda: a message published from a Lambda function with the iot-data SDK is never routed by the matching IoT topic rule to its SQS action, so the end-to-end check never converges

> _Note: this issue was identified against the code base at commit `323c298260f641866db820e53cda367b812962dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
One net-new root cause, grounded in one failing behavioral check.

NOTE: open issue 20261106T180709Z-iot-lambda-in-function-iot-data-publish-and.md describes a neighbouring cause (in-function iot-data Publish/UpdateThingShadow producing no observable effect) and even lists this check id, but the replay report did NOT mark this check as owned by it — its recorded replay key does not match this run's failure — so this cause is filed separately and that issue is named here for the fixer to reconcile.

Cause 1 of 1: the full in-function publish -> topic-rule -> SQS path never produces the delivered message.

AWS: iot:DescribeEndpoint with endpointType "iot:Data-ATS" returns the account's data endpoint address, which a client uses as the iot-data endpoint. IoT Data Plane Publish (Smithy: com.amazonaws.iotdataplane#Publish, POST /topics/{topic}, code 200; `topic` httpLabel + required; `qos` httpQuery, integer, default 0, range 0..1; `retain` httpQuery boolean default false; `payload` httpPayload blob; modeled errors InternalFailureException 500, InvalidRequestException 400, MethodNotAllowedException 405, ThrottlingException 429, UnauthorizedException 401) answers HTTP 200 with an empty body. A stored, enabled topic rule whose SQL is `SELECT * FROM '<topic>'` is then evaluated by the rules engine for that published message, and its `sqs` action with `useBase64: false` forwards exactly the published payload (the JSON document as published, unwrapped and un-base64'd) to the configured queue URL under the action's roleArn. Delivery is asynchronous and is observed on the downstream service (an SQS ReceiveMessage), never in the Publish response.

Emulator: the check never completes. The replay reports only `Error: Check timed out after 60.0s`. The check creates the queue, the rule role, the topic rule, the Lambda execution role (iot:DescribeEndpoint on *, iot:Publish on arn:aws:iot:<region>:<account>:topic/<topic>) and the python3.12 function, invokes it with a unique marker document, and then polls the queue for up to 90 s for a body whose `marker` equals that value. It is still polling when the harness budget expires: the queue never receives the marker, and no HTTP status, error code or forwarded payload is ever observed for the in-function publish. Because the invocation itself is retried past FunctionError, the check cannot distinguish "the handler's publish was refused" from "the publish was accepted and nothing was routed" — either way nothing is delivered.

Why: this is the ordinary IoT ingestion shape — telemetry published from compute, fanned out by a rule. A client sees every control-plane call succeed (queue created, rule created and readable, function Active) and the publish apparently accepted, while no message ever arrives at the rule's target. Any application whose logic depends on the rule firing silently does nothing, and a test asserting the message reached SQS hangs to its own deadline instead of failing with a refusal.

Fix: the iot-data Publish handler and the topic-rule evaluation/action-dispatch component it must drive (match the published topic against each stored rule's SQL FROM clause, skip rules with ruleDisabled=true, resolve the action's roleArn, and apply the `sqs` action by writing the un-base64'd payload to the target queue's state), plus the in-function SDK path that lets a Lambda handler reach iot/iot-data — the endpoint address DescribeEndpoint announces must be the one the function can actually reach from inside the emulator — and the IAM authorization of iot:Publish against the topic ARN (a role without it must be refused UnauthorizedException, HTTP 401, and nothing forwarded).

Traces: iot__lambda.lambda-sdk-iot-publish-positive (verdict mismatch; detail `Error: Check timed out after 60.0s`).

## Evidence
Expected: the handler resolves the data endpoint via iot:DescribeEndpoint(endpointType="iot:Data-ATS"), its iot-data Publish of the marker document answers HTTP 200 with an empty body, the function returns {"ok": true} with no FunctionError, and within the poll window the rule's SQS queue holds exactly the published payload — a single body equal to the marker document and nothing else.
Actual: the check is terminated by the harness: `Error: Check timed out after 60.0s`. The queue never yields a body carrying the marker; the assertion `matched == [{"marker": <marker>}]` is never reached. No status code, error code or error message is produced by the publish path at all — the divergence is the absent 200-then-delivery, not a wrong code. (The marker and all generated names/ARNs are per-run values, not requirements.)

## Steps to Reproduce
1. Boot the emulator. Create an SQS queue and read its QueueArn.
2. Create an IAM role trusted by iot.amazonaws.com with an inline policy allowing sqs:SendMessage on that queue ARN.
3. iot CreateTopicRule ruleName=<r> topicRulePayload={sql: "SELECT * FROM '<topic>'", awsIotSqlVersion: "2016-03-23", ruleDisabled: false, actions: [{sqs: {roleArn: <rule role arn>, queueUrl: <queue url>, useBase64: false}}]}.
4. Create an IAM role trusted by lambda.amazonaws.com with AWSLambdaBasicExecutionRole attached plus an inline policy allowing iot:DescribeEndpoint on * and iot:Publish on arn:aws:iot:<region>:<account>:topic/<topic>.
5. Create a python3.12 Lambda function whose handler calls boto3 iot.describe_endpoint(endpointType="iot:Data-ATS"), builds an iot-data client at 'https://' + endpointAddress, and calls publish(topic=<topic>, qos=0, payload=json.dumps({"marker": <marker>})), returning {"ok": True}. Wait for State == Active.
6. Invoke the function with {"region": <region>, "topic": <topic>, "payload": ...} and poll sqs ReceiveMessage on the queue for a body whose `marker` equals the published marker.
7. Observe that the queue never yields the forwarded message and the check polls until the harness kills it at 60 s. Re-replay: iot__lambda.lambda-sdk-iot-publish-positive.
