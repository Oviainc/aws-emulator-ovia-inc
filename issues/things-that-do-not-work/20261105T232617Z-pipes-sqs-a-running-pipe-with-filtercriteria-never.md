Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes__sqs.pipe-filter-criteria-non-matching-not-delivered
Issue-Key: pipes__sqs.pipe-filter-criteria-non-matching-not-delivered|_collect#31|Timeout|resp = sqs.receive_message(
Issue-Commit: ef5ad16d2372229eb93d6a23769435c8216c4367

# pipes__sqs: a RUNNING pipe with FilterCriteria never delivers the matching SQS record envelope to its target (check hangs to the harness cap)

> _Note: this issue was identified against the code base at commit `ef5ad16d2372229eb93d6a23769435c8216c4367`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
This issue has 1 net-new root cause; it must be closed before submitting.

Cause 1 of 1: a pipe whose SourceParameters carry a FilterCriteria pattern reaches CurrentState RUNNING and then moves nothing — the matching (control) record never reaches the target, so the suppression half of the check can never even be evaluated.

AWS: recorded behavior only for the EventBridge Pipes data plane (the Smithy model describes the control plane only). Smithy (com.amazonaws.pipes): CreatePipe accepts SourceParameters -> PipeSourceParameters, which for an SQS source carries FilterCriteria (structure FilterCriteria with member Filters -> FilterList, @length min 0 max 5, each Filter having Pattern -> EventPattern, @length min 0 max 4096, @sensitive) and SqsQueueParameters -> PipeSourceSqsQueueParameters (BatchSize @range 1..10000, MaximumBatchingWindowInSeconds @range 0..300); DescribePipe reports CurrentState from PipeState, whose values include RUNNING, CREATE_FAILED, START_FAILED. The documented Pipes contract is that once CurrentState is RUNNING the SERVICE polls the source with no client action, evaluates every polled record against FilterCriteria, consumes and discards non-matching records without touching the target, and delivers each matching record to the target as the SQS record envelope: a JSON object whose keys are exactly attributes, awsRegion, body, eventSource, eventSourceARN, md5OfBody, md5OfMessageAttributes, messageAttributes, messageId and receiptHandle, where body is the source body VERBATIM as a string, messageId/md5OfBody/md5OfMessageAttributes are the values SendMessage reported for the SOURCE message, eventSource is the fixed text "aws:sqs", eventSourceARN is the SOURCE queue's ARN (not the target's), awsRegion is the request's region, messageAttributes is lowercase-keyed (dataType/stringValue) and carries the source message's attributes, attributes holds exactly ApproximateFirstReceiveTimestamp, ApproximateReceiveCount, SenderId and SentTimestamp with ApproximateReceiveCount the STRING "1" on first delivery, and the target message itself carries no MessageAttributes of its own. No refusal is involved: CreatePipe, DescribePipe, IAM CreateRole and SQS SendMessage all answer 200 here, so there is no diverging status, error code or error message to quote — the divergence is an absent delivery.

Emulator: the check never completes a single envelope assertion. It creates the source and target queues, creates a role trusted by pipes.amazonaws.com with sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the source and sqs:SendMessage on the target, calls CreatePipe with SourceParameters={FilterCriteria:{Filters:[{Pattern:{"body":{"kind":["wanted"]}}}]}, SqsQueueParameters:{BatchSize:10}}, waits for DescribePipe CurrentState==RUNNING, sends one non-matching and one matching message, and then polls the target for the matching record. The replay result is `Error: Check timed out after 60.0s` — the wait never satisfies, so no record (matching or not) is ever observed at the target and the source messages are never consumed. The emulator reports the pipe as healthy throughout.

Why: a client that filters at the pipe sees the pipe report RUNNING and then observes an empty target forever. It cannot distinguish "my FilterCriteria pattern matched nothing" from "the pipe never ran at all", because both look identical: no record at the target, no message consumed at the source, no state reason, no error. Every filtered fan-in built on Pipes silently stalls, and a filter-regression test can never fail honestly because its positive control never arrives.

Fix: the pipes data plane / dispatch engine in services/service-com-amazonaws-pipes-pipes — a per-pipe source poller driven by the stored SourceParameters, a FilterCriteria evaluator applied per record, and a target dispatch stage that builds the SQS record envelope at the POLLING end (so eventSourceARN names the source queue) and sends it to the target through the emulator's sqs surface, deleting the source message only after a successful target send; plus the in-flight-batch state that backs it.

Related, not covering: the open issue 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md describes the same "a RUNNING pipe never polls or delivers" theme and lists many pipes checks, but its replay keys do not include this check (the suite did not mark this check OWNED), so this cause is still unowned work. Note also that 7 of the 9 pipes__sqs checks in this run passed, so this is not a wholesale service outage — the FilterCriteria-configured delivery path this check exercises is what does not move records.

Traces: pipes__sqs.pipe-filter-criteria-non-matching-not-delivered.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), per-service line: `pipes__sqs 7/9 (78%) 2 failed`. Failing check listed under heading `### pipes__sqs`:

  #### [mismatch] pipes__sqs.pipe-filter-criteria-non-matching-not-delivered
  Error: Check timed out after 60.0s

No bootstrap signal: the emulator was serving (622/1196 checks passed overall, and 7 of 9 pipes__sqs checks passed), so CreatePipe/DescribePipe/SendMessage were answered.

Expected (from the check's own assertions, which it never reached): after SendMessage of a body matching the pipe's FilterCriteria pattern {"body":{"kind":["wanted"]}}, at least one message appears on the target queue whose body is a JSON object with exactly the keys {attributes, awsRegion, body, eventSource, eventSourceARN, md5OfBody, md5OfMessageAttributes, messageAttributes, messageId, receiptHandle}; env["body"] == the source body verbatim; env["messageId"] == the source SendMessage MessageId; env["md5OfBody"] == the source SendMessage MD5OfMessageBody; env["md5OfMessageAttributes"] == the source SendMessage MD5OfMessageAttributes; env["eventSource"] == "aws:sqs"; env["eventSourceARN"] == the SOURCE queue ARN; env["awsRegion"] == the request region; env["messageAttributes"]["probe"]["dataType"]/["stringValue"] carry the source attribute lowercase-keyed; set(env["attributes"]) == {ApproximateFirstReceiveTimestamp, ApproximateReceiveCount, SenderId, SentTimestamp} with ApproximateReceiveCount == "1"; and the target message has no MessageAttributes of its own. Separately, the non-matching body must never appear at the target.

Actual: the target queue yields nothing within the harness cap; the check ends as `Check timed out after 60.0s` before any of the above is evaluated, and the source messages are not consumed.

The pipe name, queue URLs/ARNs, role ARN, message ids, MD5s, host, port and timestamps in this check are generated per run and are examples only. The requirement is the behavior: a RUNNING pipe with FilterCriteria consumes its source, suppresses non-matching records, and delivers matching records to the target as the SQS record envelope described above.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. Create two SQS queues: a source with VisibilityTimeout=60 and a target.
3. Create an IAM role whose trust policy names pipes.amazonaws.com, with an inline policy allowing sqs:ReceiveMessage, sqs:DeleteMessage and sqs:GetQueueAttributes on the source queue ARN and sqs:SendMessage on the target queue ARN.
4. Call CreatePipe(Name=<pipe>, RoleArn=<role arn>, Source=<source arn>, Target=<target arn>, SourceParameters={"FilterCriteria": {"Filters": [{"Pattern": "{\"body\": {\"kind\": [\"wanted\"]}}"}]}, "SqsQueueParameters": {"BatchSize": 10}}).
5. Poll DescribePipe(Name=<pipe>) until CurrentState == "RUNNING".
6. SendMessage to the source twice: body {"kind":"unwanted","marker":"suppressed"} and body {"kind":"wanted","marker":"control"}, each with MessageAttributes {"probe": {"DataType": "String", "StringValue": "attr-bad"/"attr-good"}}.
7. Poll ReceiveMessage on the TARGET queue. Observe that no message ever arrives (and that ReceiveMessage on the SOURCE still shows the two messages, i.e. the pipe never consumed them), so the wait runs to the harness cap.
8. After fixing, re-replay pipes__sqs.pipe-filter-criteria-non-matching-not-delivered and confirm the matching record arrives as the SQS record envelope with the source message's messageId/md5OfBody/md5OfMessageAttributes and eventSourceARN == the source queue ARN, while the non-matching record never reaches the target and is consumed from the source.
