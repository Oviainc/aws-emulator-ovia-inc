Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt
Issue-Key: kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt|_wait_invocation_logged#18|TimeoutError|raise TimeoutError(f"an invocation logging {marker!r}: the run budget ended first; not graded")
Issue-Commit: 9e55f581c86f2cc2a9694ee1b8e2f61584dc3f55

# kms__lambda__sqs: an Enabled SQS event source mapping never invokes its function, so the SSE-KMS withhold-until-decrypt contract is never graded (1 cause)

> _Note: this issue was identified against the code base at commit `9e55f581c86f2cc2a9694ee1b8e2f61584dc3f55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
ONE root cause, grounded in one failing behavioral check that the replay report marked neither ALREADY FILED nor OWNED.

Related open issues named for context only (neither carries this check's replay key, so neither covers it): 20261024T102105Z-kms-lambda-sqs-lambda-event-source-mappings-never-deliver-sqs-messages.md (same check id, recorded against commit aa31aa65 with an Issue-Key frame `_wait_invocation_logged#19|Timeout|time.sleep(3)` that no longer matches the check as replayed here), 20261027T180000Z-lambda-an-sqs-event-source-mapping-never-consumes-or-delivers.md and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (synchronous Invoke). The poller/withhold contract exercised here is a separate observable from synchronous Invoke, and it is still broken at this commit.

Cause 1 of 1 — an event source mapping over an SQS queue that reports State 'Enabled' never reads the queue and never invokes the mapped function, so nothing is ever written to the function's log group.
AWS: "Creates a mapping between an event source and an Lambda Event Function. Lambda reads items from the event source and invokes the function" (Smithy: lambda#CreateEventSourceMapping, POST /2015-03-31/event-source-mappings, HTTP 202, output EventSourceMappingConfiguration with State/StateTransitionReason/LastProcessingResult). Once the mapping's State is 'Enabled', the Lambda-managed poller drains the mapped queue and invokes the function with the batch; for a plain SSE-SQS queue the handler's output (here, a print of each record body) appears in /aws/lambda/<function> within seconds, and the queue's ApproximateNumberOfMessages/ApproximateNumberOfMessagesNotVisible drain to 0/0. The KMS half of the contract: when the function's execution role holds sqs:ReceiveMessage but NOT kms:Decrypt on the queue's CMK, the poller withholds the invocation and does not consume the message — it stays counted in the queue across more than two visibility timeouts — and granting kms:Decrypt on that key delivers the same message to the handler. Recorded behavior only for the withhold leg; the model states only the mapping lifecycle, not the poller's permission gate.
Emulator: CreateEventSourceMapping is accepted and both mappings (the SSE-KMS queue and the SSE-SQS twin) are observed at State 'Enabled' by GetEventSourceMapping, but no invocation ever happens for EITHER queue. The calibration leg — which only requires the plain SSE-SQS twin to deliver — fails with `TimeoutError: an invocation logging 'sim2real-behav-ssesqs-<id>': the run budget ended first; not graded` raised from the log poll, i.e. nothing matching the message body was ever present in /aws/lambda/<function> (ResourceNotFoundException on the group is treated as 'no events' by the check, so a missing group counts the same). services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/create_event_source_mapping.rs does arm `crate::state::drive_sqs_pollers` / `event_source_poller::drive_mapping` at the end of handle, but no delivery is observable afterwards; because the check then only talks to SQS, IAM and CloudWatch Logs, nothing re-drives the poll loop and no batch is ever handed to a function.
Why: every SQS-triggered Lambda pattern is unobservable while the control plane reports a healthy Enabled mapping. A client cannot test a consumer, a batch size, a redrive/DLQ path, or the production failure mode this check is about — a queue whose consumer role lacks kms:Decrypt silently stalls. Worse, the emulator cannot be distinguished from the broken case: 'no invocation' is exactly what the KMS-denied leg expects, so a client's negative test passes for the wrong reason while its positive test also sees nothing.
Fix: the Lambda event-source poller in the lambda service crate (crate::state::drive_sqs_pollers / crate::state::sqs_poller / crate::state::event_source_poller, armed from services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/create_event_source_mapping.rs) — it must actually drain an Enabled mapping's queue on its own clock, invoke the mapped function with the batch, and emit the function's output to /aws/lambda/<function>; plus the kms:Decrypt gate on the queue's CMK, which must withhold the invocation without consuming or deleting the message and release it once the role is granted kms:Decrypt.
Traces: kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt.

## Evidence
Check kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt, verdict [error]:

  File "check_kms__lambda__sqs_esm_on_sse_kms_queue_withholds_invocation_until_the_role_can_decrypt.py", line 61, in run
    assert _wait_invocation_logged(ctx, logs, fn_name, twin_marker, timeout_s=180.0), (
  File "...", line 210, in _wait_invocation_logged
    raise TimeoutError(f"an invocation logging {marker!r}: the run budget ended first; not graded")
  TimeoutError: an invocation logging 'sim2real-behav-ssesqs-24cec1f94709': the run budget ended first; not graded

Sequence completed successfully before the hang: kms CreateKey; sqs CreateQueue with Attributes {VisibilityTimeout:5, KmsMasterKeyId:<cmk arn>}; a twin sqs CreateQueue with {VisibilityTimeout:5, SqsManagedSseEnabled:'true'}; iam CreateRole + PutRolePolicy granting logs:* and sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on both queue ARNs and deliberately NO kms:Decrypt; lambda CreateFunction (python3.12, Timeout 3) observed State=='Active'; lambda CreateEventSourceMapping(EventSourceArn=<each queue>, FunctionName=<fn>, BatchSize=1) for both, each observed State=='Enabled'; sqs SendMessage to both queues.

Expected: the SSE-SQS twin's message body appears in a /aws/lambda/<fn> log stream within seconds (the check allows 180 s); then the SSE-KMS message stays counted by ApproximateNumberOfMessages+ApproximateNumberOfMessagesNotVisible >= 1 for at least two visibility timeouts with its marker never logged; then after iam PutRolePolicy granting kms:Decrypt on the key, the queue drains to 0/0 and the held marker is logged.
Actual: no log stream or event carrying either marker ever appears for the function, so the calibration leg never completes and neither the withhold leg nor the post-grant delivery leg reaches a verdict. The only non-2xx involved is the ResourceNotFoundException the log group raises because it is never created by an invocation.

## Steps to Reproduce
1. kms CreateKey (symmetric, customer-managed); note Arn.
2. sqs CreateQueue QUEUE_KMS with Attributes {'VisibilityTimeout':'5','KmsMasterKeyId':<key arn>}; sqs CreateQueue QUEUE_TWIN with {'VisibilityTimeout':'5','SqsManagedSseEnabled':'true'}; read QueueArn for both.
3. iam CreateRole trusting lambda.amazonaws.com, with an inline policy allowing logs:CreateLogGroup/CreateLogStream/PutLogEvents on arn:aws:logs:*:*:* and sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on both queue ARNs, and NO kms:Decrypt.
4. lambda CreateFunction (python3.12, Timeout=3, handler printing 'forwarded:'+record['body'] for each event Record); wait for State=='Active'.
5. lambda CreateEventSourceMapping(EventSourceArn=<QUEUE_TWIN arn>, FunctionName=<fn>, BatchSize=1) and the same for QUEUE_KMS; wait for both to report State=='Enabled'.
6. sqs SendMessage to QUEUE_TWIN with a unique body, and to QUEUE_KMS with another.
7. Poll logs DescribeLogStreams/GetLogEvents (or FilterLogEvents) on /aws/lambda/<fn> — observe that neither body is ever logged, where AWS logs the QUEUE_TWIN body within seconds and withholds only the QUEUE_KMS one.
8. iam PutRolePolicy adding kms:Decrypt on <key arn>; observe QUEUE_KMS never drains and the held body is never logged.
9. Replay kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt to confirm it no longer hits the run-budget timeout.
