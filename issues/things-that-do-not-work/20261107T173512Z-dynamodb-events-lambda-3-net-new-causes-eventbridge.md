Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__lambda.cfn-eventbridge-rule-lambda-target-deployment dynamodb__events__lambda.events-rule-role-assume-role-positive dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onsuccess-positive dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onfailure-positive dynamodb__events__lambda.ddb-stream-esm-destination-missing-permission-rejected
Issue-Key: dynamodb__events__lambda.cfn-eventbridge-rule-lambda-target-deployment|_poll_until#9|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: dynamodb__events__lambda.events-rule-role-assume-role-positive|_poll_until#35|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onsuccess-positive|_poll_until#23|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
Issue-Key: dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onfailure-positive|_poll_until#22|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
Issue-Key: dynamodb__events__lambda.ddb-stream-esm-destination-missing-permission-rejected|_receive_one#17|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=5)
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# dynamodb__events__lambda: 3 net-new causes — EventBridge rules never invoke their Lambda target, Lambda async-invoke destinations never deliver to EventBridge, and an ESM destination-permission check never settles

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Three independent root causes in this scenario group; each is listed with its own failing check ids and all three must be closed before this issue is submitted.

Cause 1 of 3: an EventBridge rule with a Lambda target never invokes the target function.
AWS: eventbridge PutRule + PutTargets (Targets[].Arn = a function ARN) causes every event matching the rule's EventPattern (or every scheduled firing) to be delivered to the function as its invocation event, provided the function has a lambda:InvokeFunction resource policy for events.amazonaws.com (or the rule's RoleArn is assumable). A handler that writes a marker item to DynamoDB therefore produces that item shortly after PutEvents.
Emulator: the target is never invoked — the marker item never appears: `item marker-<id> written by EventBridge-invoked Lambda not met within 60.0s. Last result: None`, and the rule-role variant likewise never produces the item (`condition not met within 60.0s; last=None`). The rule and target are accepted, so nothing signals the break.
Why: every event-driven fan-out through EventBridge silently does nothing; a client sees a healthy rule with a target and no delivery, and cannot tell a wiring error from a non-delivering bus.
Fix: the eventbridge rule-matching/dispatch path for Lambda targets (and its assumption of the rule's RoleArn), which must invoke the function with the matched event.
Traces: dynamodb__events__lambda.cfn-eventbridge-rule-lambda-target-deployment, .events-rule-role-assume-role-positive. Related open issue that does not carry these checks' replay keys: 20261029T180026Z-dynamodb-events-lambda-eventbridge-rules-never-invoke-their-lambda-targets-ddb.md.

Cause 2 of 3: Lambda asynchronous-invoke destinations never deliver to an EventBridge event bus.
AWS: Smithy: lambda PutFunctionEventInvokeConfig with DestinationConfig.OnSuccess.Destination / OnFailure.Destination set to an event-bus ARN. After an async Invoke (InvocationType=Event), Lambda publishes an invocation record to that bus — on success once the handler returns, and on failure after MaximumRetryAttempts is exhausted — with a detail-type of `Lambda Function Invocation Result - Success` / `… - Failure` and a detail carrying requestContext (condition SUCCESS/RetriesExhausted), requestPayload and responsePayload. A rule on that bus therefore receives the record.
Emulator: neither destination ever produces a record on the bus — both the OnSuccess and the OnFailure check poll and see nothing (`condition not met within 90.0s (last=None)` and `condition not met within 120.0s (last=None)`).
Whi/Why: a client relying on async destinations for success/failure routing loses every invocation record, so failures are undetectable and success-triggered workflows never start.
Fix: the lambda async-invocation pipeline's destination emitter — after the retry policy resolves, it must publish the invocation record to the configured OnSuccess/OnFailure destination.
Traces: dynamodb__events__lambda.lambda-async-invoke-destination-eventbridge-onsuccess-positive, .lambda-async-invoke-destination-eventbridge-onfailure-positive. Related open issue that does not carry these checks' replay keys: 20261107T042814Z-dynamodb-events-lambda-lambda-asynchronous-invoke-destinations-never.md.

Cause 3 of 3: a DynamoDB-stream ESM whose OnFailure destination is not permitted never reaches an observable outcome.
AWS: Smithy: lambda CreateEventSourceMapping DestinationConfig.OnFailure.Destination. When the poller may not write to the configured on-failure destination, the failing batch's discard is still accounted for on the mapping (LastProcessingResult / the mapping's state reflect the delivery problem) and polling continues; the refusal is observable rather than silent.
Emulator: the check never reaches any verdict and is killed at the harness cap (`Check timed out after 180.0s`) — neither a delivered failure record nor a recorded refusal ever appears.
Why: a client cannot discover that its on-failure destination is mis-permissioned; the mapping looks healthy while failed batches vanish.
Fix: the lambda event-source-mapping poller's on-failure destination path — it must attempt the destination write, record the refusal on the mapping, and continue polling.
Traces: dynamodb__events__lambda.ddb-stream-esm-destination-missing-permission-rejected.

## Evidence
Cause 1 — expected: the EventBridge-invoked function writes the marker item within seconds of PutEvents. Actual: `AssertionError: item marker-1af74ce2a13c written by EventBridge-invoked Lambda not met within 60.0s. Last result: None`; `AssertionError: condition not met within 60.0s; last=None` (rule RoleArn variant).

Cause 2 — expected: an invocation record on the configured event bus (detail-type `Lambda Function Invocation Result - Success` / `- Failure`, detail.requestContext.condition SUCCESS / RetriesExhausted). Actual: `AssertionError: condition not met within 90.0s (last=None)` (OnSuccess) and `AssertionError: condition not met within 120.0s (last=None)` (OnFailure).

Cause 3 — expected: an observable outcome for a failing batch whose OnFailure destination is unpermitted. Actual: `Error: Check timed out after 180.0s`.

## Steps to Reproduce
Cause 1: PutRule with an EventPattern, PutTargets naming a function that writes a DynamoDB marker item, add the lambda:InvokeFunction permission for events.amazonaws.com, PutEvents a matching event, then poll for the item. Cause 2: PutFunctionEventInvokeConfig with DestinationConfig.OnSuccess/OnFailure pointing at a custom event bus, add a rule on that bus capturing the record, Invoke(InvocationType=Event) with a succeeding and then a failing handler, and poll for the record. Cause 3: create a DynamoDB-stream ESM with an OnFailure destination the execution role may not write, write a poison record, and poll for the mapping's outcome. Re-replay the five checks in trace_ids.
