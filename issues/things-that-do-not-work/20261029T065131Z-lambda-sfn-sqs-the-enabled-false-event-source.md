Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.sqs-esm-disabled-mapping-no-invocation-negative
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-disabled-mapping-no-invocation-negative|run#20|Timeout|resp = sqs.receive_message(QueueUrl=results["url"], MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 6c829b3943454ada5df5dfe2630a7a4c6694ce1c

# lambda__sfn__sqs: the Enabled=False event source mapping check makes no progress - an SQS mapping neither accounts for the parked message nor delivers it once re-enabled (1 cause)

> _Note: this issue was identified against the code base at commit `6c829b3943454ada5df5dfe2630a7a4c6694ce1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
ONE cause in this issue; it must be closed before submitting.

Related open issues: 20261027T180000Z-lambda-an-sqs-event-source-mapping-never-consumes-or-delivers.md and 20261028T004031Z-lambda-sfn-sqs-a-fifo-sqs-event-source.md (which owns the sibling FIFO check in this same group) describe the SQS event-source-mapping delivery gap, but the replay suite does NOT report this check as OWNED by either, so it is filed here with its own replay key. Closing those issues without re-replaying this check leaves the Enabled=False/re-enable contract unverified.

Cause 1 of 1 - an SQS event source mapping has no poller, so the disabled-then-enabled lifecycle produces no observable effect and the check cannot converge inside the harness cap.
AWS: CreateEventSourceMapping with an SQS EventSourceArn and Enabled=False returns 202 with State 'Disabled' and polls nothing; a message sent while the mapping is disabled stays on the source queue, so GetQueueAttributes(ApproximateNumberOfMessages) settles at 1 (the counter is eventually consistent, which is why it is polled). UpdateEventSourceMapping(UUID, Enabled=True) returns 202 and the mapping transitions through Enabling to State 'Enabled' (Smithy: CreateEventSourceMapping POST /2015-03-31/event-source-mappings -> 202, output EventSourceMappingConfiguration carrying State/StateTransitionReason/LastProcessingResult; UpdateEventSourceMapping PUT .../{UUID} -> 202 same output; GetEventSourceMapping GET .../{UUID} -> 200; errors InvalidParameterValueException 400 / ResourceNotFoundException 404 / ResourceConflictException 409 / ResourceInUseException 400 - i.e. the accepted calls above are 202/200 and must take effect). Once Enabled, the poller receives the parked message and invokes the function with a Records batch whose body is the parked message body, after which the source queue drains to ApproximateNumberOfMessages 0.
Emulator: the check makes no progress and is killed by the harness at the 60 s cap (verdict mismatch, detail 'Check timed out after 60.0s'), even though its own waits budget 45 s + 90 s + 60 s + 90 s + 90 s. No invocation ever occurs on either leg - the negative leg is satisfied only vacuously (nothing is ever delivered, disabled or not), and the positive control that re-enables the SAME mapping and requires the parked message to arrive in the results queue, and the source queue's ApproximateNumberOfMessages to settle at 1 while disabled and 0 after delivery, is never reached.
Why: a client cannot distinguish 'the mapping is correctly disabled' from 'mappings never deliver at all' - the single most common SQS->Lambda consumption pattern, including the Enabled flag that operators use to park and resume a consumer, has no observable behavior. Pipelines that park a queue during an incident and resume it afterwards appear to work and silently process nothing, and the queue-depth signals used to monitor that are never driven.
Fix: the Lambda event-source-mapping runtime in the lambda service crate (services/service-com-amazonaws-lambda-awsgirapiservice) - the mapping's stored State must drive a poller that receives from the SQS source queue, invokes the target function with the Records envelope, and deletes the consumed messages, so the source queue's ApproximateNumberOfMessages moves 1 -> 0 across a disable/enable transition; the state transitions (Disabled/Enabling/Enabled) already reported must gate that poller rather than being inert.
Traces: lambda__sfn__sqs.sqs-esm-disabled-mapping-no-invocation-negative.

## Evidence
Suite report, service row lambda__sfn__sqs 4/14 (29%), 10 failed. This check is reported with verdict 'mismatch' and detail 'Check timed out after 60.0s'. It is NOT marked ALREADY FILED and NOT marked OWNED by any open issue (its sibling lambda__sfn__sqs.sqs-esm-fifo-queue-message-group-ordering-preserved IS owned by 20261028T004031Z).

The check's own sequence and budgets: _create_esm(..., BatchSize=10, Enabled=False) then assert get_event_source_mapping(UUID)['State'] == 'Disabled'; send_message(MessageBody='parked-while-disabled'); a 45 s receive_message loop on the results queue asserting NO invocation; _poll_until(get_queue_attributes(['ApproximateNumberOfMessages']) == 1, timeout_s=90); update_event_source_mapping(UUID, Enabled=True) + _poll_until(State == 'Enabled', timeout_s=60); a 90 s receive loop requiring the delivered body to equal 'parked-while-daisabled'/'parked-while-disabled'; _poll_until(ApproximateNumberOfMessages == 0, timeout_s=90).
Expected on AWS: the whole sequence converges - 1 visible message while disabled, the parked body delivered after re-enabling, 0 visible afterwards. Actual: the run is still inside those polls when the harness's 60 s cap kills it; no results-queue message is ever produced on either leg.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. sqs CreateQueue twice (a source and a results queue); iam CreateRole for a lambda execution role allowing sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the source and sqs:SendMessage on the results queue.
3. lambda CreateFunction with a handler that, for each record in event['Records'], sends {'body': record['body']} to the results queue.
4. lambda CreateEventSourceMapping(EventSourceArn=<source arn>, FunctionName=<fn>, BatchSize=10, Enabled=False); GetEventSourceMapping reports State 'Disabled'.
5. sqs SendMessage(QueueUrl=<source>, MessageBody='parked-while-disabled'). Expect no invocation and GetQueueAttributes(ApproximateNumberOfMessages) on the source to settle at 1.
6. lambda UpdateEventSourceMapping(UUID, Enabled=True); poll GetEventSourceMapping until State is 'Enabled'. Expect the parked message to be delivered (a results-queue message whose body is 'parked-while-disabled') and the source queue's ApproximateNumberOfMessages to settle at 0. Observe: nothing is ever delivered and the source queue's counters never move, so the check never converges.
