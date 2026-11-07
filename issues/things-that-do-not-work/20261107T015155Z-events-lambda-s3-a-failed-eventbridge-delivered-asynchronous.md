Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3.eventbridge-lambda-failure-routes-to-dlq
Issue-Key: events__lambda__s3.eventbridge-lambda-failure-routes-to-dlq|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Commit: 569322fd02c2429e9947d96bd3e38eb76e62453b

# events__lambda__s3: a failed EventBridge-delivered asynchronous Lambda invocation never reaches the function's OnFailure destination (1 cause)

> _Note: this issue was identified against the code base at commit `569322fd02c2429e9947d96bd3e38eb76e62453b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFunctionEventInvokeConfig

## Description
One net-new root cause in Lambda's asynchronous-invocation failure handling.

Cause 1 of 1: a function with an event-invoke config whose DestinationConfig.OnFailure names an SQS queue, invoked asynchronously by an EventBridge rule target and raising in its handler, never produces an OnFailure record on that queue.

AWS: lambda PutFunctionEventInvokeConfig (Smithy: PUT /2019-09-25/functions/{FunctionName}/event-invoke-config, members MaximumRetryAttempts @range 0-2, MaximumEventAgeInSeconds @range 60-21600, DestinationConfig) records the failure destination for asynchronous invocations. An EventBridge rule whose target is a Lambda function invokes it asynchronously, so a handler exception consumes the retry budget and, with MaximumRetryAttempts=0, immediately exhausts it; Lambda then delivers an invocation record to DestinationConfig.OnFailure.Destination. That record is a JSON document carrying requestContext.condition == "RetriesExhausted" and a responsePayload that reproduces the handler's raised error (errorType/errorMessage), alongside requestContext.requestId and requestContext.functionArn. The fixed part of the contract is the member names and the literal condition value "RetriesExhausted"; the variable parts are the request id, the function ARN and the handler's own message text, all taken from the invocation.

Emulator: no message ever arrives on the OnFailure queue. The check re-publishes the matching event on every poll (to defeat rule-propagation races) and receives from the queue with WaitTimeSeconds=5 for up to 180 s; it is killed at the 60 s harness cap with "Check timed out after 60.0s", having never received a message. Because no record is delivered, neither requestContext.condition nor responsePayload is produced at all. Hypothesis: either the EventBridge rule never performs the asynchronous Invoke, or the asynchronous invoke path discards the handler failure instead of consulting the stored DestinationConfig; the replay cannot distinguish the two, but in both cases the OnFailure destination is never written.

Why: a client that builds an EventBridge -> Lambda pipeline with a dead-letter/OnFailure destination loses every failed event silently. The queue a client monitors for poison events stays empty, so a broken handler looks identical to an idle pipeline, and any retry/replay tooling built on the OnFailure record has nothing to consume.

Fix: the asynchronous-invocation path of the Lambda invoke handler — the component that, after the retry budget for an Event-type invocation is exhausted, serialises the invocation record and delivers it to the function's stored DestinationConfig.OnFailure target — together with the EventBridge target dispatcher that must issue that asynchronous Invoke for a rule whose target is a function ARN.

Note: open issue 20261025T002357Z-events-lambda-s3-an-eventbridge-rule-s-lambda-target-is-never.md describes a neighbouring symptom for this service, but the replay does not mark this check as owned by it, and the destination-delivery contract above is not covered there.

Traces: events__lambda__s3.eventbridge-lambda-failure-routes-to-dlq.

## Evidence
Check: events__lambda__s3.eventbridge-lambda-failure-routes-to-dlq [mismatch]
Error: Check timed out after 60.0s

Expected (AWS): after PutRule(EventPattern {"source":["check.fail"],"detail-type":["FailEvent"]}) + PutTargets(<function ARN>) + AddPermission(Principal events.amazonaws.com) and PutFunctionEventInvokeConfig(MaximumRetryAttempts=0, DestinationConfig={"OnFailure":{"Destination":<queue ARN>}}), a PutEvents entry matching the rule leads to an SQS message whose Body parses to a JSON record with requestContext.condition == "RetriesExhausted" and a responsePayload containing the handler's raised text ("intentional failure for DLQ test").

Actual (emulator): ReceiveMessage on the OnFailure queue returns no messages for the whole run; the check's own poll (timeout 180 s, interval 5 s, each round re-publishing the event) is cut off by the 60 s harness cap. The assertions on requestContext.condition and responsePayload are therefore never reached, because no record is delivered at all.

## Steps to Reproduce
1. Create an SQS queue and set a queue policy allowing lambda.amazonaws.com sqs:SendMessage on it, conditioned on aws:SourceArn == the function ARN.
2. Create a python Lambda whose handler does `raise RuntimeError("intentional failure for DLQ test")`, with an execution role granting sqs:SendMessage on the queue ARN.
3. lambda PutFunctionEventInvokeConfig(FunctionName=<fn>, MaximumRetryAttempts=0, DestinationConfig={"OnFailure": {"Destination": <queue ARN>}}).
4. events PutRule with EventPattern {"source":["check.fail"],"detail-type":["FailEvent"]}; lambda AddPermission(Principal=events.amazonaws.com, SourceArn=<rule ARN>); events PutTargets(Rule=<rule>, Targets=[{Id:"t", Arn:<function ARN>}]).
5. events PutEvents([{Source:"check.fail", DetailType:"FailEvent", Detail:"{}"}]), then poll sqs ReceiveMessage(QueueUrl=<queue>, WaitTimeSeconds=5), re-publishing each round.
   Expected: a message arrives whose Body has requestContext.condition == "RetriesExhausted" and a responsePayload carrying the raised error text.
   Actual: the queue stays empty until the check is killed at the 60 s cap.
