Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.e2e-topic-rule-destination-confirm-then-deliver
Issue-Key: iot.e2e-topic-rule-destination-confirm-then-deliver|run#31|AssertionError|assert _find_record_with_marker(ddb, table_name, direct_probe_marker) is not None, (
Issue-Commit: 63c64ce081b9c54ca8821134bfa1b94c44bc83e7

# iot: an in-function boto3 DynamoDB PutItem from a python3.12 Lambda never persists, so the HTTP topic-rule-destination confirm-then-deliver flow cannot even arm its recorder

> _Note: this issue was identified against the code base at commit `63c64ce081b9c54ca8821134bfa1b94c44bc83e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a python3.12 Lambda handler whose execution role grants dynamodb:PutItem on the target table runs to completion in the emulator, but its in-process boto3 DynamoDB write raises, so nothing is persisted and the handler returns its own caught-exception result. This is the very first, diagnostic step of the IoT HTTP-topic-rule-destination end-to-end check (a direct Invoke of the recorder function, with API Gateway and IoT entirely out of the path), so the check can never go on to exercise CreateTopicRuleDestination / ConfirmTopicRuleDestination / HTTP action delivery at all.

AWS: Smithy (lambda): Invoke -> POST /2015-03-31/functions/{FunctionName}/invocations, output StatusCode bound to the HTTP response code, FunctionError bound to X-Amz-Function-Error, Payload as the httpPayload. On AWS the managed python3.x runtime ships boto3/botocore, so `boto3.resource("dynamodb").Table(t).put_item(Item={...})` from a handler whose role allows dynamodb:PutItem on that table succeeds: the invoke returns 200 with no X-Amz-Function-Error and a payload of {"statusCode": 200, "body": "ok"} (the handler's own success branch), and the item is subsequently visible to DynamoDB Scan/GetItem on the table. Recorded behavior only for the handler-level success/"ok" contract and for the runtime's bundled-package list; the in-function failure here carries no modeled error code, because the SDK call is made inside the execution environment, not over the public Invoke API.

Emulator: the direct Invoke returns 200 with no FunctionError (so the module imported and the handler ran), but the payload is the handler's except-branch result {"statusCode": 200, "body": "error-recorded"} — i.e. boto3's DynamoDB put_item raised inside the execution environment. A following DynamoDB Scan of the table finds no item whose body contains the probe marker; the handler's fallback "<requestId>-error" record is not observable either, so the nested write failed the same way and the exception text never escapes the sandbox. The replay therefore fails on: AssertionError: recorder Lambda ran but did not persist the direct-invoke probe: {'statusCode': 200, 'body': 'error-recorded'}.

Why: a client gets a 200 Invoke with no FunctionError — the Lambda API's only signal that the handler failed — while the handler's AWS-side effect never happened. Any emulated workload that stores from inside a function (and here, any IoT HTTP-destination/recorder pattern that proves delivery by persisting what it received) silently loses its writes and looks healthy; worse, it hides the real subject of the test, so IoT HTTP topic rule destination confirmation and post-confirmation delivery are never exercised.

Fix: the python execution environment behind the lambda service's Invoke handler — the in-guest AWS SDK and the credential/endpoint wiring it hands the handler for calls back into the emulator (the DynamoDB resource/client path in particular), plus surfacing the guest exception instead of swallowing it. Several open issues report in-function SDK failures under other service headings (for example 20261106T062901Z-dynamodb-lambda-lambda-python-runtime-has-no-boto3-so-the.md, which is the import-time variant, and 20261106T151148Z-dynamodb-lambda-s3-an-in-function-dynamodb-putitem.md); none of them carries this check, which the suite lists under `iot`.

Traces: iot.e2e-topic-rule-destination-confirm-then-deliver.

## Evidence
Expected (AWS): lam.invoke(FunctionName=<recorder>, Payload={"body": "{\"probe\": \"<marker>\"}"}) -> HTTP 200, no X-Amz-Function-Error, payload {"statusCode": 200, "body": "ok"}; a subsequent dynamodb:Scan of the recorder table returns an item whose `body` attribute contains <marker>.
Actual (emulator): HTTP 200, no X-Amz-Function-Error, payload {"statusCode": 200, "body": "error-recorded"} (the handler's except branch), and the Scan returns no item containing <marker> and no "<requestId>-error" record. Replay assertion: `AssertionError: recorder Lambda ran but did not persist the direct-invoke probe: {'statusCode': 200, 'body': 'error-recorded'}` (check_iot_e2e_topic_rule_destination_confirm_then_deliver.py line 90). In the message, "recorder Lambda ran but did not persist the direct-invoke probe: " is the check's fixed text and the trailing dict is the emulator's actual Invoke payload; the marker value is generated per run and is not part of the requirement.

## Steps to Reproduce
1. Replay the behavioral suite and re-run iot.e2e-topic-rule-destination-confirm-then-deliver.
2. Or reproduce directly: iam:CreateRole trusting lambda.amazonaws.com, attach AWSLambdaBasicExecutionRole and an inline policy allowing dynamodb:PutItem on arn:aws:dynamodb:<region>:*:table/<table>; dynamodb:CreateTable (<table>, HASH key `id` of type S, PAY_PER_REQUEST) and wait for table_exists.
3. lambda:CreateFunction with Runtime=python3.12, Handler=recorder.handler, whose code does `boto3.resource("dynamodb").Table("<table>").put_item(Item={"id": context.aws_request_id, "ts": <int>, "body": event.get("body")})` and returns {"statusCode":200,"body":"ok"}, catching any exception and returning {"statusCode":200,"body":"error-recorded"}; wait for function_active_v2.
4. lambda:Invoke it with Payload={"body": "{\"probe\": \"probe-1\"}"} and read the response payload: the emulator returns body "error-recorded" instead of "ok".
5. dynamodb:Scan <table>: no item's `body` contains "probe-1" on the emulator; on AWS the item is there.
