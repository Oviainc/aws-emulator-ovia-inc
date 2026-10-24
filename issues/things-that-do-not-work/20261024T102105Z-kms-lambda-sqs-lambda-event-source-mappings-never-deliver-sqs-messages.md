Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt
Issue-Key: kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt|_wait_invocation_logged#19|Timeout|time.sleep(3)
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# Lambda event source mappings never deliver SQS messages to the function, so the ESM/KMS withhold check hangs to the 60 s cap at its delivery calibration (1 cause)

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1 — an Enabled SQS event source mapping never invokes its function, so no poller-driven delivery can be observed.
AWS: an ESM over an SQS queue polls the queue and invokes the function with the message batch; for an SSE-SQS queue the function's log shows the message body within seconds. On an SSE-KMS queue whose function role holds sqs:ReceiveMessage but no kms:Decrypt on the queue's CMK, the Lambda-managed poller WITHHOLDS the invocation and does not lose the message — it stays in flight or becomes visible again across more than two visibility timeouts — and granting kms:Decrypt lets the same message reach the handler.
Emulator: CreateEventSourceMapping succeeds and the mapping reaches state 'Enabled', but the SSE-SQS calibration message never reaches the function: the check ends as 'Check timed out after 60.0s' while awaiting the twin queue's invocation in the function's log group (its own budget is 180 s). With no delivering twin the withhold claim cannot be graded at all, so no verdict was reached for the KMS half either.
Why: the entire SQS-triggered Lambda pattern is unobservable — a client cannot test a consumer, a batch size, a DLQ or the KMS-permission failure mode that silently stalls a production queue. The emulator reports a healthy Enabled mapping while delivering nothing.
Fix: the Lambda event-source poller (the component that drains the mapped queue and invokes the function), plus the kms:Decrypt gate that must withhold the invocation without consuming the message. Related and already open: '20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md' (synchronous Invoke cannot run a deployment package) — it may share a root cause but does not carry this check's replay key, and the poller/withhold semantics are a separate contract from Invoke.

## Evidence
kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt: 'Error: Check timed out after 60.0s'. Sequence before the hang: _create_queue(KmsMasterKeyId=<cmk>) and a twin _create_queue(SqsManagedSseEnabled='true'); create_event_source_mapping(EventSourceArn=..., FunctionName=..., BatchSize=1) for both, both observed 'Enabled' via _wait_esm_state; send_message to both; then _wait_invocation_logged(logs, fn, twin_marker, timeout_s=180.0) never observes the SSE-SQS twin's invocation. Expected: the twin's marker appears in the function's CloudWatch Logs within seconds, and the SSE-KMS message is withheld (never invoked, never deleted) until kms:Decrypt is granted.

## Steps to Reproduce
1. kms:CreateKey. 2. sqs:CreateQueue with KmsMasterKeyId=<key>, plus a twin queue with SqsManagedSseEnabled=true. 3. iam:CreateRole granting sqs:ReceiveMessage on both queues and no kms:Decrypt. 4. lambda:CreateFunction with that role. 5. lambda:CreateEventSourceMapping for each queue, BatchSize=1; wait for 'Enabled'. 6. sqs:SendMessage to both. 7. Poll the function's log group for the twin's body — nothing is ever logged. 8. Then grant kms:Decrypt and confirm the withheld message reaches the handler.
