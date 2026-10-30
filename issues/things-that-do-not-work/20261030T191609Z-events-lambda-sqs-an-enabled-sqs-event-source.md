Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.cli-delete-event-source-mapping-stops-polling
Issue-Key: events__lambda__sqs.cli-delete-event-source-mapping-stops-polling|_drain_queue_for#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: ae59a9f600d99b40a154d5dcee0e42433113c02e

# events__lambda__sqs: an enabled SQS event-source mapping never invokes the function, so `delete-event-source-mapping` stops-polling cannot be observed (1 cause)

> _Note: this issue was identified against the code base at commit `ae59a9f600d99b40a154d5dcee0e42433113c02e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One root cause. Cause 1 of 1: an enabled Lambda event-source mapping whose EventSourceArn is an SQS queue never polls the queue and never invokes the function, so the check's own positive control (the pre-delete relay) never completes and the whole check runs out of time before its negative half is ever graded.

AWS: Smithy (service-com-amazonaws-lambda-awsgirapiservice) CreateEventSourceMapping POST /2015-03-31/event-source-mappings -> 202 EventSourceMappingConfiguration with UUID/State/BatchSize/EventSourceArn/FunctionArn; DeleteEventSourceMapping DELETE /2015-03-31/event-source-mappings/{UUID} -> 202; UpdateEventSourceMapping PUT /2015-03-31/event-source-mappings/{UUID} -> 202 with modeled ResourceNotFoundException (httpError 404). Once the mapping reports State "Enabled", Lambda's managed poller calls sqs:ReceiveMessage on the source queue and synchronously invokes the function with an `event` of the form {"Records": [{"body": <the SendMessage MessageBody>, "messageId": ..., "eventSource": "aws:sqs", "eventSourceARN": <the queue ARN>, ...}]}, deleting each successfully processed message. After DeleteEventSourceMapping the poller stops, a subsequently sent message is never relayed and stays receivable on the source queue, and UpdateEventSourceMapping against the deleted UUID is refused 404 ResourceNotFoundException. The number of records per invoke is bounded by the request's BatchSize (here 1) — BatchSize is a value from the request, the rule (poll-and-invoke while Enabled, stop when deleted) is the requirement.

Emulator: CreateEventSourceMapping is accepted and GetEventSourceMapping reports State "Enabled", but nothing is ever polled or invoked: the sink queue the relay handler writes to stays empty. The check's positive control (`_drain_queue_for(sink, body.Records[0].body == marker1)`, 60 s budget) never matches, so the check ends "Error: Check timed out after 60.0s" inside `_drain_queue_for` and never reaches its post-delete assertions (the 404 on UpdateEventSourceMapping, the held absence window, the sink depth of 0, or that the post-delete message stays on the source queue). The mapping surface therefore reports an enabled, correctly-wired poller that does no work.

Why: a client that wires SQS to Lambda through an event-source mapping gets a resource that reports Enabled and accepts every configuration knob while no message is ever processed; the queue silently fills. Worse for test suites, the negative assertion ("deletion stops polling") is unverifiable here because the positive control it is calibrated against never fires, so a later regression in the delete path would look identical to today's behaviour.

Fix: the Lambda event-source-mapping poller runtime for SQS sources (poll loop, BatchSize batching, record-envelope construction, synchronous invoke, delete-on-success) and the teardown of that poller on DeleteEventSourceMapping; the mapping's State in the CreateEventSourceMapping/GetEventSourceMapping handlers must not report "Enabled" for a mapping that nothing drives.

Related open issue, named for the fixer: 20261029T182228Z-events-lambda-sqs-eventbridge-sqs-targets-ignore-the.md (its Cause 4 describes the same never-invoking SQS ESM for other checks). That issue does NOT own this replay key — the suite reported this check unowned — so it is filed here as its own work item and should be closed together with that cause.

Traces: events__lambda__sqs.cli-delete-event-source-mapping-stops-polling.

## Evidence
Failing check: events__lambda__sqs.cli-delete-event-source-mapping-stops-polling — verdict mismatch, "Error: Check timed out after 60.0s".

Expected (AWS): after `aws lambda create-event-source-mapping --function-name <fn> --event-source-arn <queue arn> --batch-size 1` reaches State "Enabled", a `sqs send-message` of body `<marker>` to the source queue causes one invoke of the relay handler, which forwards `json.dumps(event)` to the sink queue; the sink therefore yields a message whose JSON body has Records[0].body == `<marker>`. Then `aws lambda delete-event-source-mapping --uuid <uuid>` makes GetEventSourceMapping answer 404 ResourceNotFoundException, `UpdateEventSourceMapping(UUID=<uuid>, BatchSize=2)` answer 404 ResourceNotFoundException, a second `send-message` never reach the sink (sink ApproximateNumberOfMessages == "0"), and that second body remain receivable on the source queue.

Actual (emulator): the create call is accepted and the mapping reports State "Enabled", but the sink queue never produces any message at all. The check's positive-control drain loop (`_drain_queue_for`, WaitTimeSeconds=5 receive_message in a loop) never matches and the check is killed at the 60 s budget. Because the positive control is what calibrates the later absence window, none of the post-delete assertions above were evaluated. Queue URLs, function names, UUIDs and the `before-`/`after-` markers in the recording are per-run generated values; the requirement is that an Enabled SQS mapping produces exactly one relayed record per sent message and that deletion stops it.

## Steps to Reproduce
1. CreateQueue twice (a `source` and a `sink`).
2. CreateRole trusting lambda.amazonaws.com with sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the source queue and sqs:SendMessage on the sink queue.
3. CreateFunction (python3.12) whose handler does `sqs.send_message(QueueUrl=os.environ["SINK_QUEUE_URL"], MessageBody=json.dumps(event))`; wait for function_active_v2.
4. `aws lambda create-event-source-mapping --function-name <fn> --event-source-arn <source queue arn> --batch-size 1`; poll GetEventSourceMapping until State == "Enabled".
5. `aws sqs send-message --queue-url <source url> --message-body before-X`.
6. Poll ReceiveMessage on the sink queue for up to 60 s — expected: one message whose JSON body has Records[0].body == "before-X". Observed: the sink queue stays empty indefinitely.
7. (Never reached in the emulator) `aws lambda delete-event-source-mapping --uuid <uuid>`; then UpdateEventSourceMapping on that UUID must answer 404 ResourceNotFoundException, a second send-message must not reach the sink, and it must remain receivable on the source queue.
