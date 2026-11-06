Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__lambda.lambda-sdk-iot-publish-positive iot__lambda.lambda-sdk-iot-publish-missing-permission-negative iot__lambda.lambda-sdk-iot-update-thing-shadow-positive
Issue-Key: iot__lambda.lambda-sdk-iot-publish-positive|_invoke#11|Timeout|resp = lam.invoke(FunctionName=fn_name, Payload=json.dumps(payload).encode("utf-8"))
Issue-Key: iot__lambda.lambda-sdk-iot-publish-missing-permission-negative|_invoke_until_delivered#13|Timeout|time.sleep(4.0)
Issue-Key: iot__lambda.lambda-sdk-iot-update-thing-shadow-positive|_invoke#10|Timeout|resp = lam.invoke(FunctionName=fn_name, Payload=json.dumps(payload).encode("utf-8"))
Issue-Commit: f50d8ce9e37ffa71976cdbe00198995770403bcc

# iot__lambda: in-function iot-data Publish and UpdateThingShadow never produce an observable effect, so all three lambda-sdk IoT data-plane checks hang to the harness cap

> _Note: this issue was identified against the code base at commit `f50d8ce9e37ffa71976cdbe00198995770403bcc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
One net-new root cause, covering three failing checks.

Cause 1 of 1: a Lambda handler that resolves the data endpoint with iot:DescribeEndpoint and then calls the IoT data plane (iot-data Publish, UpdateThingShadow) never produces the effect the check polls for — the published message never reaches the topic rule's SQS action, the shadow update is never read back, and the un-permitted variant never surfaces a refusal — so each check polls until the 60 s harness cap kills it. Failing check ids: iot__lambda.lambda-sdk-iot-publish-positive, iot__lambda.lambda-sdk-iot-publish-missing-permission-negative, iot__lambda.lambda-sdk-iot-update-thing-shadow-positive.

AWS: iot:DescribeEndpoint with endpointType "iot:Data-ATS" returns the account's data endpoint address. IoT Data Plane Publish is POST /topics/{topic} (Smithy: topic is an httpLabel and required; qos is an httpQuery with default 0 and range 0..1; payload is the httpPayload blob) and answers HTTP 200 with an empty body; the published message is then matched by a topic rule whose SQL selects that topic, and an sqs action with useBase64 false forwards exactly the published payload to the queue. UpdateThingShadow is POST /things/{thingName}/shadow (Smithy: thingName httpLabel required, pattern ^[a-zA-Z0-9:_-]+$; payload httpPayload required) and answers HTTP 200 with the merged state document as its payload. A caller whose role lacks iot:Publish on the topic ARN is refused UnauthorizedException (Smithy: httpError 401) and the message is NOT forwarded.

Emulator: none of the three checks completes. Each replay reports only `Error: Check timed out after 60.0s`. The positive publish check invokes the function and then polls the queue for up to 90 s for a message carrying the published marker; the shadow check polls for the updated shadow document; the negative check polls for the refusal and for the absence of a forwarded message. All three are still polling when the harness budget expires, so no HTTP status, no error code and no forwarded payload is ever observed.

Why: a client that publishes telemetry from a Lambda function — the ordinary IoT ingestion shape — sees neither a delivered message nor an error. The publish appears to be accepted while nothing is routed, so a rule-driven pipeline silently drops everything, and the negative case gives the caller no refusal to distinguish "not authorized" from "delivered".

Fix: the IoT data-plane Publish and UpdateThingShadow handlers and the topic-rule matching/action dispatch that must forward a matched message to the rule's sqs target, plus the in-function SDK path that lets a handler reach iot/iot-data (DescribeEndpoint resolution and the data endpoint the emulator announces) and the IAM authorization of iot:Publish against the topic ARN. The open issues 20261029T175031Z-iot-lambda-no-iot-topic-rule-with-a.md, 20261105T114938Z-iot-lambda-the-python-lambda-runtime-provides-no.md and 20261105T223150Z-iot-lambda-2-net-new-causes-the-managed.md describe neighbouring IoT->Lambda and runtime-SDK causes, but none of them carries these three checks' replay keys.

Traces: iot__lambda.lambda-sdk-iot-publish-positive, iot__lambda.lambda-sdk-iot-publish-missing-permission-negative, iot__lambda.lambda-sdk-iot-update-thing-shadow-positive (all verdict mismatch; detail `Error: Check timed out after 60.0s`).

## Evidence
Expected: the handler's iot-data publish answers 200, the function returns {"ok": true}, and within the poll window the rule's SQS queue holds exactly the published payload (the marker document and nothing else); UpdateThingShadow answers 200 with the merged state document; with iot:Publish withheld the handler's call is refused UnauthorizedException (401) and no message is forwarded.
Actual: all three checks are terminated by the harness with `Error: Check timed out after 60.0s` — the queue never receives the marker, the shadow is never observable, and no refusal is ever surfaced.

## Steps to Reproduce
1. Create an SQS queue and an IoT topic rule whose SQL is SELECT * FROM '<topic>' with an sqs action (useBase64 false) and a rule role allowed to send to the queue.
2. Create a python Lambda function whose role allows iot:DescribeEndpoint on * and iot:Publish on arn:aws:iot:<region>:<account>:topic/<topic>, with a handler that calls iot.describe_endpoint(endpointType="iot:Data-ATS") and then iot-data publish(topic=..., qos=0, payload=...).
3. Invoke the function with a unique marker payload and poll the queue for the forwarded message.
4. Repeat with iot:Publish removed from the role (expect UnauthorizedException and no forwarded message), and with a handler calling UpdateThingShadow on a created thing.
5. Observe that every variant polls without converging until the harness kills it at 60 s.
