Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix
Issue-Key: dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix|_poll_until#31|Timeout|time.sleep(interval_s)
Issue-Commit: e177fe1c5dfe93ced4cde9b5eb5c9137f1b03615

# dynamodb__lambda__sqs: the SQS poison-message -> DLQ -> StartMessageMoveTask -> reprocess round trip still produces no observable effect, so the redrive check runs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `e177fe1c5dfe93ced4cde9b5eb5c9137f1b03615`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause on the dynamodb__lambda__sqs service token. The failing check is marked neither ALREADY FILED nor OWNED in the replay report, so no open issue carries its current replay key. Related open/closed issues that do NOT cover it: 20261026T094219Z-dynamodb-lambda-sqs-an-sqs-event-source-mapping.md (open) names this same check id in its Issue-Trace-Ids but carries a DIFFERENT replay key for it (`...|_poll_until#20|Timeout|time.sleep(interval_s)`), which is why the report no longer attributes this failure to it — the check now stalls at a different poll site; 20261025T065401Z-sqs-never-redrives-to-a-dead-letter-queue.md (open) is filed against the `sqs` token; and the fixed issues 20261027T135029Z-sqs-startmessagemovetask-never-completes-a-dlq-drain-no.md and 20261028T004031Z-events-lambda-sqs-an-sqs-event-source-mapping.md claimed the single-service forms of the move-task drain and SQS event-source consumption. This check exercises the composite SQS->Lambda->DynamoDB operator-recovery flow and still makes no progress.

Every wait in this check is a POLL that returns as soon as the effect appears (the check's own helpers `_poll_until`/`_wait_for_effect`/`_retry_until_accepted` with 2-5 s intervals); there is no unconditional long sleep. Against a faithful emulator, where propagation is immediate, the whole check finishes in seconds. Running to the harness cap therefore means some leg produced no effect at all, not that the check is intrinsically slow.

Cause 1 of 1: the poison-message redrive round trip never advances.
Failing check: dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix

AWS: the check drives one contract made of modelled calls, each of which AWS answers with an observable effect.
(a) CreateEventSourceMapping (Smithy: com.amazonaws.lambda#CreateEventSourceMapping, POST /2015-03-31/event-source-mappings, success code 202, output EventSourceMappingConfiguration carrying UUID/State/LastProcessingResult; errors InvalidParameterValueException httpError 400, ResourceConflictException 409, ResourceNotFoundException 404, TooManyRequestsException 429, ServiceException 500) is REFUSED when the source queue's VisibilityTimeout is below the function's Timeout, with Error.Code `InvalidParameterValueException`, HTTP 400 and message `Queue visibility timeout: <n> seconds is less than Function timeout: <m> seconds` — the fixed text is `Queue visibility timeout: ` / ` seconds is less than Function timeout: ` / ` seconds`, and <n>/<m> are taken from the queue's VisibilityTimeout attribute and the function's configured Timeout in the request, never from any one run. The refused call must leave no mapping behind, i.e. ListEventSourceMappings for that (FunctionName, EventSourceArn) pair stays empty.
(b) Once the VisibilityTimeout is legal, the accepted mapping must actually poll the queue and invoke the function, so a handler that raises makes the message's receive count advance.
(c) A source queue carrying RedrivePolicy {deadLetterTargetArn, maxReceiveCount: 1} must move the message to the dead-letter queue after that many failed deliveries, so the DLQ's ApproximateNumberOfMessages rises above 0.
(d) StartMessageMoveTask (Smithy: com.amazonaws.sqs#StartMessageMoveTask; required SourceArn, optional DestinationArn/MaxNumberOfMessagesPerSecond; output TaskHandle; errors InvalidAddress 404, InvalidSecurity 403, RequestThrottled 403, ResourceNotFoundException 404, UnsupportedOperation 400 / awsQueryError AWS.SimpleQueueService.UnsupportedOperation) must actually MOVE the dead-lettered messages back, and ListMessageMoveTasks (output Results of ListMessageMoveTasksResultEntry, whose Status is documented as one of RUNNING, COMPLETED, CANCELLING, CANCELLED, FAILED, with ApproximateNumberOfMessagesMoved a Long defaulting to 0 and SourceArn echoing the request) must report the task reaching Status COMPLETED with ApproximateNumberOfMessagesMoved >= 1 and SourceArn equal to the DLQ ARN.
(e) After the move the repaired consumer must reprocess the message (its DynamoDB PutItem of `processed:<marker>` lands) and the DLQ must drain back to ApproximateNumberOfMessages 0.

Emulator: the check never reaches a verdict on ANY of (a)-(e). The only thing the replay records is `Check timed out after 60.0s`, so no HTTP status, no error code and no error message is produced for the ESM refusal, and no DLQ depth, task Status or moved count is produced for the redrive. Hypothesis (neither the model nor the recording states where it stalls): the earliest site is the read-back poll of the shortened VisibilityTimeout after SetQueueAttributes; the next are the ESM's `State == Enabled` poll, the DLQ-depth poll (nothing is ever dead-lettered because nothing is ever delivered/retried), and the ListMessageMoveTasks terminal-status poll. A fixer should re-replay this one check id and read which poll description is the one that exhausts its budget.

Why: a client relying on AWS's behaviour loses the entire standard operator recovery for a poisoned event pipeline. The queue-to-function wiring reports success while consuming nothing, so no message ever reaches the dead-letter queue; StartMessageMoveTask hands back a TaskHandle while moving nothing, so a recovery runbook appears to succeed with the messages still stranded and ListMessageMoveTasks never reaching COMPLETED; and because the VisibilityTimeout-below-function-Timeout refusal is never observed, a client can build a wiring AWS rejects outright, in which a redelivery races a still-running invocation.

Fix: the Lambda event-source-mapping poller/dispatch component on the SQS source path (ReceiveMessage/Invoke/DeleteMessage driving the mapping, not the CreateEventSourceMapping request handler, which already accepts the call), the CreateEventSourceMapping validation that compares the source queue's VisibilityTimeout against the function's Timeout, and the SQS queue state's receive-count/RedrivePolicy dead-lettering step plus the StartMessageMoveTask/ListMessageMoveTasks task executor. The composite path these components form is what this check grades.

Traces: dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix

## Evidence
Suite: simparity-behavioral-ga-prod-v1, 234/451 passed (51.9%). Service dynamodb__lambda__sqs: 0/5, 5 failed; four of the five are OWNED by open issues, this one is not.

[mismatch] dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix
  Error: Check timed out after 60.0s

Expected (AWS):
  - CreateEventSourceMapping(FunctionName=<fn with Timeout 5>, EventSourceArn=<queue with VisibilityTimeout 2>, BatchSize=1, Enabled=True) is refused: Error.Code 'InvalidParameterValueException', HTTP 400, Error.Message 'Queue visibility timeout: 2 seconds is less than Function timeout: 5 seconds' (fixed text 'Queue visibility timeout: ... seconds is less than Function timeout: ... seconds'; the two numbers are the queue attribute and the function Timeout from the request/state, not literals of one run), and ListEventSourceMappings(FunctionName, EventSourceArn) returns EventSourceMappings == [].
  - With VisibilityTimeout restored to >= the function Timeout, the mapping is accepted and reaches State 'Enabled'.
  - The poison message, rejected by the handler, exceeds maxReceiveCount=1 and lands on the DLQ: GetQueueAttributes(DLQ, ['ApproximateNumberOfMessages']) >= 1, while GetItem for 'processed:<marker>' in the control table returns no Item.
  - StartMessageMoveTask(SourceArn=<dlq-arn>) returns a non-empty TaskHandle; ListMessageMoveTasks(SourceArn=<dlq-arn>).Results[0] reaches Status 'COMPLETED' with SourceArn == <dlq-arn> and ApproximateNumberOfMessagesMoved >= 1.
  - After the move the repaired consumer writes 'processed:<marker>' and the DLQ drains to ApproximateNumberOfMessages == 0.

Actual (emulator replay): no leg produces an observable effect; the check exhausts the harness's 60 s cap with the sole verdict 'Check timed out after 60.0s'. No status code, error code, error message, DLQ depth, task Status or moved count is emitted by the emulator for any of the assertions above, because execution never gets past a poll that never satisfies.

Why this is not check-intrinsic latency: every wait is a poll (helpers `_poll_until`, `_wait_for_effect`, `_retry_until_accepted`, intervals 2-5 s) that returns the moment the effect appears; the check's own budgets (60/90/120/180 s) are upper bounds, not sleeps.

Relation to already-filed work: open 20261026T094219Z-dynamodb-lambda-sqs-an-sqs-event-source-mapping.md lists this check id but holds a different replay key for it (`_poll_until#20|Timeout|time.sleep(interval_s)`), so the report does not mark this failure OWNED; fixed 20261027T135029Z-sqs-startmessagemovetask-never-completes-a-dlq-drain-no.md and fixed 20261028T004031Z-events-lambda-sqs-an-sqs-event-source-mapping.md addressed the single-token forms of the move-task drain and SQS ESM consumption, and this composite flow still makes no progress.

## Steps to Reproduce
1. git_clone the repo and boot the emulator (the behavioral suite builds and boots it).
2. Re-replay exactly: dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix, and record WHICH poll description exhausts its budget ('the shortened VisibilityTimeout is readable', 'CreateEventSourceMapping accepted once the VisibilityTimeout propagates', 'ESM <uuid> Enabled', 'poison message on the DLQ', 'message move task terminal status', 'ApproximateNumberOfMessagesMoved counter to catch up to the COMPLETED task', 'redriven message <marker> reprocessed', or 'DLQ drained by the move task').
3. Minimal reproduction by hand:
   a. CreateQueue dlq; CreateQueue main with VisibilityTimeout=6 and RedrivePolicy {"deadLetterTargetArn": <dlq-arn>, "maxReceiveCount": 1}.
   b. CreateTable control (PAY_PER_REQUEST, pk/S); PutItem {pk: {S: 'mode'}, v: {S: 'broken'}}.
   c. CreateRole trusting lambda.amazonaws.com allowing sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on main and dynamodb:GetItem/PutItem on control; CreateFunction (python3.12, Timeout=5) whose handler reads 'mode' from control, raises when it is 'broken', and otherwise PutItems 'processed:<marker>'; wait until State == 'Active'.
   d. SetQueueAttributes(main, VisibilityTimeout='2'), then GetQueueAttributes(main, ['VisibilityTimeout']) and confirm it reads back '2'.
   e. CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<main-arn>, BatchSize=1, Enabled=True) -> must be InvalidParameterValueException / HTTP 400 / 'Queue visibility timeout: 2 seconds is less than Function timeout: 5 seconds'; then ListEventSourceMappings(FunctionName, EventSourceArn) must be empty.
   f. SetQueueAttributes(main, VisibilityTimeout='6'); CreateEventSourceMapping again and poll GetEventSourceMapping until State == 'Enabled'.
   g. SendMessage(main, {"marker": "<marker>"}); poll GetQueueAttributes(dlq, ['ApproximateNumberOfMessages']) for >= 1, and confirm GetItem(control, {pk: 'processed:<marker>'}) has no Item.
   h. PutItem {pk: 'mode', v: 'fixed'}; StartMessageMoveTask(SourceArn=<dlq-arn>) and poll ListMessageMoveTasks(SourceArn=<dlq-arn>) for Status 'COMPLETED' with ApproximateNumberOfMessagesMoved >= 1 and SourceArn == <dlq-arn>.
   i. Poll GetItem(control, {pk: 'processed:<marker>'}) for the item, and GetQueueAttributes(dlq, ['ApproximateNumberOfMessages']) for 0.
