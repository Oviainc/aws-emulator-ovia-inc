Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__sqs.revoked-downstream-permission-is-denied-with-the-exact-authz-message-and-the-message-lands-in-the-dlq dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix
Issue-Key: dynamodb__lambda__sqs.revoked-downstream-permission-is-denied-with-the-exact-authz-message-and-the-message-lands-in-the-dlq|_poll_item#39|Timeout|time.sleep(2)
Issue-Key: dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: ac01852cc17d3270a54b28950effcfcf66838250

# dynamodb__lambda__sqs: an SQS event source mapping never invokes the function and the DLQ/redrive state machine never advances — both fault-injection checks run to the 60 s cap (2 causes)

> _Note: this issue was identified against the code base at commit `ac01852cc17d3270a54b28950effcfcf66838250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Two net-new root causes in the SQS -> Lambda -> DynamoDB event pipeline for the dynamodb__lambda__sqs service token. Neither failing check is marked ALREADY FILED or OWNED in the replay report, and no open issue carries their replay keys: the nearest open issues are 20261025T022656Z-lambda-s3-sqs-sqs-event-source-mappings-never.md and 20261025T002357Z-events-lambda-sqs-eventbridge-lambda-targets-and-sqs-event-source-mappings.md, which are filed against the lambda__s3__sqs and events__lambda__sqs tokens and do not cover these checks.

Both checks fail as 'Check timed out after 60.0s'. Every wait inside them is a POLL that returns as soon as the effect appears (the suite report shows no unconditional sleep longer than 5 s), so against a faithful emulator — where propagation is immediate — each check completes in seconds. Running to the harness cap therefore means a step produced no effect at all, not that the check is intrinsically slow.

=== Cause 1 of 2: an SQS event source mapping never delivers a message to its function ===
Failing check: dynamodb__lambda__sqs.revoked-downstream-permission-is-denied-with-the-exact-authz-message-and-the-message-lands-in-the-dlq

AWS: CreateEventSourceMapping (Smithy: com.amazonaws.lambda#CreateEventSourceMapping, POST /2015-03-31/event-source-mappings, success code 202; output EventSourceMappingConfiguration with State/LastProcessingResult; errors InvalidParameterValueException 400, ResourceConflictException 409, ResourceNotFoundException 404, TooManyRequestsException 429, ServiceException 500) wires a poller that, once the mapping's State reaches 'Enabled', receives messages from the source queue and synchronously invokes the function with an SQS event whose Records carry body, messageId and attributes.ApproximateReceiveCount. The check's baseline leg sends one message and polls GetItem for the item the handler writes with a 180 s budget; on real AWS (recorded in the check header against us-west-2) that item appears, then IAM revocation is observed, then the message is redriven.

Emulator: the mapping is created and the check proceeds, but no effect of a delivery is ever observable: the handler's DynamoDB PutItem never lands, so the baseline poll never satisfies and the check exhausts the harness's 60 s cap with the verdict 'Check timed out after 60.0s'. Nothing downstream of the first delivery (the per-request IAM re-evaluation, the AccessDeniedException authz message, the receive-count progression) is reachable.

Why: a client that wires a queue to a function with CreateEventSourceMapping gets a mapping that reports itself Enabled while consuming nothing. Queue depth never drains, the function never runs, and any pipeline built on SQS-triggered Lambda silently does no work — the single most common serverless consumer topology is inert, and an operator has no error to diagnose because the control plane reported success.

Fix: the Lambda event-source-mapping poller/dispatch component (the SQS source path) and its invocation of the function — not the CreateEventSourceMapping request handler, which already accepts the call. Hypothesis: the mapping is persisted as state only, with no poller driving ReceiveMessage/Invoke/DeleteMessage.

Traces: dynamodb__lambda__sqs.revoked-downstream-permission-is-denied-with-the-exact-authz-message-and-the-message-lands-in-the-dlq

=== Cause 2 of 2: the SQS redrive machinery produces no observable effect — no DLQ landing and no completed message-move task ===
Failing check: dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix

AWS: a queue carrying RedrivePolicy {deadLetterTargetArn, maxReceiveCount} moves a message to the dead-letter queue after exactly maxReceiveCount deliveries, and the dead-lettered message's ApproximateReceiveCount carries the source-queue receives over. StartMessageMoveTask (Smithy: com.amazonaws.sqs#StartMessageMoveTask, required SourceArn, returns TaskHandle; errors InvalidAddress 404, InvalidSecurity 403, RequestThrottled 403, ResourceNotFoundException 404, UnsupportedOperation 400 / awsQueryError AWS.SimpleQueueService.UnsupportedOperation) must actually MOVE the messages, so a subsequent ListMessageMoveTasks (output Results of ListMessageMoveTasksResultEntry) reports the task Status COMPLETED with a non-zero approximate-number-of-messages-moved, and the repaired consumer reprocesses the message. The check also asserts a modeled refusal on the same path: CreateEventSourceMapping on a source queue whose VisibilityTimeout (2 s) is below the function Timeout (5 s) must be refused with InvalidParameterValueException (httpError 400) and message 'Queue visibility timeout: 2 seconds is less than Function timeout: 5 seconds' — fixed text 'Queue visibility timeout: ... seconds is less than Function timeout: ... seconds', with both numbers taken from the queue attribute and the function configuration in the request — and must leave no mapping behind (ListEventSourceMappings for that function/queue pair stays empty).

Emulator: the check never reaches a verdict on any of those assertions; it runs to the 60 s cap ('Check timed out after 60.0s'), so the poison message's DLQ arrival, the move task's COMPLETED status and the reprocessing after the fix are all unobservable. Hypothesis: the stall is the same inert SQS poller as Cause 1 (the poison message is never delivered, so maxReceiveCount is never exhausted and nothing is ever dead-lettered); the earliest alternative stall point this check has is the read-back poll of the shortened VisibilityTimeout after SetQueueAttributes. A fixer should confirm which by replaying this check id once Cause 1 is fixed. The report carries only the timeout verdict, so neither a status, an error code nor a message is recorded for this cause.

Why: redrive is the standard operator recovery for a poisoned event pipeline. An emulator that accepts StartMessageMoveTask and hands back a TaskHandle while moving nothing lets a customer's recovery runbook appear to succeed with the messages still stranded; and a mapping that AWS refuses outright (visibility timeout below function timeout) being accepted lets a client build a wiring where a redelivery races a still-running invocation.

Fix: the SQS redrive/dead-letter state machine and the StartMessageMoveTask / ListMessageMoveTasks task executor in the SQS service, plus the CreateEventSourceMapping validation that compares the source queue's VisibilityTimeout against the function's Timeout.

Traces: dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix

## Evidence
Suite: simparity-behavioral-ga-prod-v1, 154/458 passed. Service dynamodb__lambda__sqs: 0/5, 5 failed; three of the five are OWNED by open issues, these two are not.

[mismatch] dynamodb__lambda__sqs.revoked-downstream-permission-is-denied-with-the-exact-authz-message-and-the-message-lands-in-the-dlq
  Error: Check timed out after 60.0s
  Expected (recorded against real AWS us-west-2, from the check header): the baseline message sent to the source queue is delivered by the event source mapping, the handler's PutItem lands, and GetItem returns {'pk': {'S': 'A'}, 'payload': {'S': 'alpha'}} within the 180 s baseline budget; after iam:DeleteRolePolicy the next delivery is denied with Error.Code 'AccessDeniedException', HTTPStatusCode 400 and message 'User: arn:aws:sts::<account>:assumed-role/<role>/<function> is not authorized to perform: dynamodb:PutItem on resource: <table-arn> because no identity-based policy allows the dynamodb:PutItem action' (fixed text as quoted; principal and resource ARN taken from the caller's session and the request's table); the message is delivered exactly maxReceiveCount=2 times with ApproximateReceiveCount '1' and '2', then lands in the DLQ with ApproximateReceiveCount '3'.
  Actual: no step after CreateEventSourceMapping produces an effect — the baseline item never appears, so the check exhausts the harness cap. No status, error code or message is produced by the emulator for the denial, because no invocation ever happens.

[mismatch] dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix
  Error: Check timed out after 60.0s
  Expected: CreateEventSourceMapping is refused with InvalidParameterValueException / HTTP 400 / 'Queue visibility timeout: 2 seconds is less than Function timeout: 5 seconds' and leaves ListEventSourceMappings empty for that pair; with a legal VisibilityTimeout=6 and maxReceiveCount=1 the poison message reaches the DLQ within seconds; StartMessageMoveTask moves it back and the task reaches Status COMPLETED with a non-zero moved count; the repaired consumer writes 'processed:<marker>' into the control table.
  Actual: the check never reaches a verdict — it runs to the 60 s cap with no observable effect on any leg.

Both checks poll (never sleep) for every effect, so a faithful emulator finishes them in seconds; the timeout is the emulator producing no effect, not check-intrinsic latency.

## Steps to Reproduce
1. git clone the repo and boot the emulator.
2. Replay check dynamodb__lambda__sqs.revoked-downstream-permission-is-denied-with-the-exact-authz-message-and-the-message-lands-in-the-dlq.
   Minimal reproduction of Cause 1: CreateTable (PAY_PER_REQUEST, pk/S); CreateQueue src with VisibilityTimeout 20 and RedrivePolicy {deadLetterTargetArn: <dlq-arn>, maxReceiveCount: 2}; CreateRole trusting lambda.amazonaws.com with sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the queue and dynamodb:PutItem on the table; CreateFunction (python3.12) whose handler PutItems each record's body; wait for function_active_v2; CreateEventSourceMapping(EventSourceArn=<src-arn>, FunctionName=<fn>, BatchSize=1) and poll GetEventSourceMapping until State == 'Enabled'; SendMessage {"pk":"A","payload":"alpha"}; then poll GetItem(TableName=<table>, Key={pk:{S:'A'}}). Observe the item never appears.
3. Replay check dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix.
   Minimal reproduction of Cause 2: with the same rig but a function Timeout of 5 s, SetQueueAttributes VisibilityTimeout=2 on the source queue and call CreateEventSourceMapping — observe whether it is refused with InvalidParameterValueException and the message quoting both timeouts, and whether ListEventSourceMappings stays empty. Then restore VisibilityTimeout=6 with maxReceiveCount=1, wire the mapping, send a message the consumer rejects, and poll ReceiveMessage on the DLQ; then StartMessageMoveTask(SourceArn=<dlq-arn>) and poll ListMessageMoveTasks(SourceArn=<dlq-arn>) for Status COMPLETED with a non-zero moved count.
