Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.cli-create-event-source-mapping-sqs-lambda events__lambda__sqs.cli-lambda-add-permission-for-eventbridge-invoke events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts
Issue-Key: events__lambda__sqs.cli-create-event-source-mapping-sqs-lambda|_drain_queue_for#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.cli-lambda-add-permission-for-eventbridge-invoke|_drain_queue_for#19|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts|_poll_until#32|Timeout|last = probe()
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# events__lambda__sqs: an SQS event-source mapping never consumes, an added lambda permission never enables rule delivery, and MaximumEventAgeInSeconds never dead-letters (3 causes)

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Three net-new root causes in this service group.

Cause 1 of 3: an SQS event-source mapping created with `aws lambda create-event-source-mapping` never consumes — a message sent afterwards is not delivered to the mapped function.
Failing check: events__lambda__sqs.cli-create-event-source-mapping-sqs-lambda.
AWS: once CreateEventSourceMapping on an SQS queue reports State Enabled, the Lambda poller receives messages from that queue and invokes the function with an `Records` batch (eventSource `aws:sqs`); a message sent after the mapping exists is processed and removed from the queue. Smithy (lambda): CreateEventSourceMapping models EventSourceArn, FunctionName, Enabled, BatchSize and returns UUID/State, so the mapping is a modeled, accepted resource; the polling behaviour is recorded behavior only.
Emulator: `Check timed out after 60.0s` — the mapping is created but the function is never invoked, the message is never processed and the sink queue the handler writes to stays empty.
Why: the standard queue-worker pattern does nothing: messages accumulate on the source queue while the mapping reports Enabled, with no error anywhere.
Fix: the SQS event-source-mapping poller in the lambda crate — an enabled SQS mapping must poll its source queue, batch records per BatchSize, invoke the function with the aws:sqs record envelope and delete the successfully-handled messages.

Cause 2 of 3: after `aws lambda add-permission` grants events.amazonaws.com scoped to the rule, the next matching event still does not invoke the function.
Failing check: events__lambda__sqs.cli-lambda-add-permission-for-eventbridge-invoke.
AWS: a rule never invokes a Lambda target that has no resource-policy statement for events.amazonaws.com; once AddPermission adds `lambda:InvokeFunction` for principal events.amazonaws.com with SourceArn = the rule ARN, the next matching event DOES invoke the function. Smithy (lambda): AddPermission models Action, Principal, SourceArn and StatementId, so the grant is a modeled write; the authorization effect is recorded behavior only.
Emulator: `Check timed out after 60.0s` — the function is not invoked before OR after the grant, so the permission boundary is unobservable and the negative half passes for the wrong reason.
Why: the fix an operator applies when deliveries are being denied (grant the rule) has no effect here, so permission-related triage is impossible and a missing grant cannot be distinguished from a broken delivery path.
Fix: the EventBridge Lambda-target delivery path in the events crate — authorize each delivery against the function's current resource policy (principal events.amazonaws.com, SourceArn = the rule's ARN) and then invoke; the decision must be re-read per delivery so a later AddPermission takes effect.

Cause 3 of 3: a target's MaximumEventAgeInSeconds never dead-letters an event — the AGE condition is not evaluated, so a retryable failure neither stops nor is recorded.
Failing check: events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts.
AWS: for a target whose RetryPolicy sets MaximumEventAgeInSeconds = 60 with a large MaximumRetryAttempts, an event whose invocations keep failing with a retryable HTTP 500 is dead-lettered once the event's age passes the limit, with NO further attempt afterwards, and the dead-letter attributes record the AGE condition; a twin rule with MaximumRetryAttempts = 0 dead-letters after RETRY_ATTEMPTS = 0 instead. Smithy (eventbridge): Target.RetryPolicy{MaximumRetryAttempts, MaximumEventAgeInSeconds} and Target.DeadLetterConfig.Arn are modeled members the emulator accepts.
Emulator: `Check timed out after 60.0s` — the DLQ receives nothing for either rule, so neither the AGE stop condition nor the retry-attempts stop condition is observable.
Why: a client that bounds event age to avoid stale processing gets neither the bound nor the dead letter; failed events vanish with no record of why delivery stopped.
Fix: the target retry/failure path in the events crate — evaluate both stop conditions (age since the event was published, and attempts against MaximumRetryAttempts) around each retryable failure, stop retrying when either trips, and write the dead letter to DeadLetterConfig.Arn with the attributes naming the condition that stopped it.
Traces: the three failing check ids listed above.

## Evidence
Cause 1 — Expected: after `aws lambda create-event-source-mapping` on the source queue, a subsequently sent message is delivered to and processed by the mapped function (observable on the sink queue). Actual: `Check timed out after 60.0s` — no invocation, sink empty.
Cause 2 — Expected: no invocation before the grant; after `aws lambda add-permission --principal events.amazonaws.com --source-arn <rule ARN> --action lambda:InvokeFunction`, the next matching event invokes the function. Actual: `Check timed out after 60.0s` — no invocation either side of the grant.
Cause 3 — Expected: the event is dead-lettered by the AGE condition (MaximumEventAgeInSeconds=60) with no later attempt, while the twin rule with MaximumRetryAttempts=0 dead-letters after RETRY_ATTEMPTS=0. Actual: `Check timed out after 60.0s` — the DLQ stays empty for both rules.

## Steps to Reproduce
Cause 1: create a source queue, a sink queue and a function (role allowing sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes and sqs:SendMessage to the sink); run `aws lambda create-event-source-mapping --event-source-arn <source queue arn> --function-name <fn>`; wait for State Enabled; SendMessage to the source and poll the sink.
Cause 2: create a rule and a Lambda target with NO resource policy; PutEvents a matching event (expect no invocation); run `aws lambda add-permission --statement-id s1 --action lambda:InvokeFunction --principal events.amazonaws.com --source-arn <rule arn>`; PutEvents again and poll for the invocation.
Cause 3: put two rules on the same pattern, each with a Lambda target that always returns HTTP 500 and a DeadLetterConfig queue — rule A with RetryPolicy{MaximumEventAgeInSeconds:60, MaximumRetryAttempts:<large>}, rule B with MaximumRetryAttempts:0; PutEvents one matching event and poll both DLQs and the handler's hit queue.
