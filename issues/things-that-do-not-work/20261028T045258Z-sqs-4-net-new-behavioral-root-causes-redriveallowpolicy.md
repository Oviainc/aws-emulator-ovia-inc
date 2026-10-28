Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.deploy-cfn-fifo-dlq-redrive-allow-policy-admits-only-the-stack-source sqs.redrive-allow-policy-denyall-blocks-automatic-redrive-on-already-wired-source sqs.message-move-task-redrives-dlq-back-to-source sqs.start-message-move-task-self-referential-destination-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.deploy-cfn-fifo-dlq-redrive-allow-policy-admits-only-the-stack-source|_error#31|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.redrive-allow-policy-denyall-blocks-automatic-redrive-on-already-wired-source|run#20|AssertionError|assert len(msgs) == 1 and msgs[0]["Body"] == body, (
Issue-Key: sqs.message-move-task-redrives-dlq-back-to-source|_poll_receive_one#19|AssertionError|raise AssertionError(f"no message received from {url} within {timeout_s}s")
Issue-Key: sqs.start-message-move-task-self-referential-destination-rejected|_error#14|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 382dee363cb5e63d68b224885f839d2dd8c7668e

# sqs: 4 net-new behavioral root causes — RedriveAllowPolicy is enforced at neither write time nor redrive time, and StartMessageMoveTask neither validates a self-referential destination nor actually moves anything

> _Note: this issue was identified against the code base at commit `382dee363cb5e63d68b224885f839d2dd8c7668e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartMessageMoveTask

## Description
This issue bundles 4 distinct net-new root causes in the SQS crate. A fixer must close all four before submitting.

=== Cause 1 of 4: SetQueueAttributes accepts a RedrivePolicy whose dead-letter target's RedriveAllowPolicy does not admit the calling queue ===
Failing check: sqs.deploy-cfn-fifo-dlq-redrive-allow-policy-admits-only-the-stack-source

AWS: Smithy — SetQueueAttributes(QueueUrl required, Attributes required) returns Unit and models InvalidAttributeValue (client, httpError 400) plus the awsQuery-mapped refusals; the RedrivePolicy / RedriveAllowPolicy pair is carried in the QueueAttributeMap. AWS evaluates the dead-letter queue's RedriveAllowPolicy at the moment a source queue tries to associate itself with that DLQ: when the DLQ's policy is {"redrivePermission":"byQueue","sourceQueueArns":[...]} and the calling queue's ARN is not in that list, the association is refused with error code InvalidParameterValue, HTTP 400, and the message
  "Value RedrivePolicy(maxReceiveCount=<count>, deadLetterTargetArn=<dlq arn>) for parameter RedrivePolicy is invalid. Reason: The value for the dead-letter queue's RedriveAllowPolicy attribute prevents you from completing this request."
Fixed text of that message: everything outside the angle brackets, i.e. "Value RedrivePolicy(maxReceiveCount=", ", deadLetterTargetArn=", ") for parameter RedrivePolicy is invalid. Reason: The value for the dead-letter queue's RedriveAllowPolicy attribute prevents you from completing this request.". Variable parts taken from the request: <count> is the maxReceiveCount the caller submitted and <dlq arn> is the deadLetterTargetArn the caller submitted. Nothing in the message comes from the emulator's own run-time state, so there is no per-run value in it.

Emulator: the call is accepted. The replay shows SetQueueAttributes returning HTTP 200 with an empty body (content-type application/x-amz-json-1.0, content-length 2) for a FIFO queue outside the stack naming a DLQ whose RedriveAllowPolicy lists only the stack's source queue, and a following GetQueueAttributes(All) then reports the forbidden RedrivePolicy as stored. The DLQ's RedriveAllowPolicy is written and read back faithfully but is never consulted on the association path.

Why: RedriveAllowPolicy is the only mechanism that keeps an unrelated queue from attaching itself to someone else's dead-letter queue. A client that relies on it — a CloudFormation stack that scopes its FIFO DLQ to exactly its own source queue, or any multi-tenant account that fences DLQs per team — gets a silent success where AWS refuses, so foreign queues end up dead-lettering into a DLQ they were explicitly denied, and the negative test in the deploying client's own suite passes against the emulator and fails in production.

Fix: the RedrivePolicy validation step of the SetQueueAttributes handler in the SQS crate (and the same validation on the CreateQueue attribute path), which must resolve the deadLetterTargetArn to its queue, read that queue's RedriveAllowPolicy, and refuse with InvalidParameterValue / 400 and the message template above when redrivePermission is byQueue and the caller's ARN is absent (or when it is denyAll).

Related but NOT covering this cause: open issue 20261025T065401Z-sqs-never-redrives-to-a-dead-letter-queue.md bundles redrive validation causes, and 20261028T014019Z-sqs-cross-region-redrivepolicy-is-refused-with-the.md covers the cross-region Reason text. Neither carries this check's replay key, and this check's FIFO/byQueue association refusal is not among their causes.

Traces: sqs.deploy-cfn-fifo-dlq-redrive-allow-policy-admits-only-the-stack-source

=== Cause 2 of 4: RedriveAllowPolicy=denyAll on the dead-letter queue does not block the automatic maxReceiveCount redirect from an already-wired source ===
Failing check: sqs.redrive-allow-policy-denyall-blocks-automatic-redrive-on-already-wired-source

AWS: recorded behavior only for the enforcement point — the Smithy model carries RedriveAllowPolicy only as a QueueAttributeName member with no prose constraint. AWS evaluates the dead-letter queue's RedriveAllowPolicy on every automatic redrive, not only when the association is created: once the DLQ's policy is set to {"redrivePermission":"denyAll"}, a message on an already-wired source queue is NOT redirected when its receive count passes maxReceiveCount. It keeps being redelivered from the source queue indefinitely, and a direct ReceiveMessage on the DLQ returns no messages. Restoring {"redrivePermission":"byQueue","sourceQueueArns":[<source arn>]} makes the very next over-count receive redirect the identical message, which then appears on the DLQ.

Emulator: with the DLQ at denyAll and the source wired with maxReceiveCount=1, the message stops being delivered from the source queue. The replay's third over-count ReceiveMessage on the source returns an empty Messages list where the body was expected: "expected '<token>-stuck' still delivered from the SOURCE (never redirected), got []". The redirect is performed as if the DLQ permitted it, so denyAll is stored (GetQueueAttributes reads it back correctly) but is never evaluated by the redrive step.

Why: denyAll is the documented emergency brake for a DLQ that is being flooded — an operator sets it to stop traffic without unwiring every source. A client relying on it gets the opposite of the guarantee: messages still leave the source queue and land in the DLQ it just closed, the source consumer stops seeing the retries it was written to handle, and there is no error anywhere to signal the policy was ignored.

Fix: the receive-count/redrive step in the ReceiveMessage path of the SQS queue state, which must re-read the dead-letter target's RedriveAllowPolicy at redrive time and withhold the move (leaving the message visible on the source) when redrivePermission is denyAll, or when it is byQueue and the source ARN is not listed.

Traces: sqs.redrive-allow-policy-denyall-blocks-automatic-redrive-on-already-wired-source

=== Cause 3 of 4: StartMessageMoveTask reaches Status=COMPLETED without moving any message and without draining its source ===
Failing check: sqs.message-move-task-redrives-dlq-back-to-source

AWS: Smithy — StartMessageMoveTask(SourceArn required, DestinationArn, MaxNumberOfMessagesPerSecond) returns TaskHandle; ListMessageMoveTasks(SourceArn required) returns Results whose entries carry Status in (RUNNING, COMPLETED, CANCELLING, CANCELLED, FAILED), ApproximateNumberOfMessagesMoved ("the approximate number of messages already moved to the destination queue") and ApproximateNumberOfMessagesToMove. A COMPLETED task is therefore a statement that the movement actually happened: every message that was on the task's source queue at sizing time is now receivable on the destination queue, and the source of the move reports ApproximateNumberOfMessages=0.

Emulator: the task does transition to COMPLETED — the replay's ListMessageMoveTasks poll settles on 'COMPLETED' inside its 90 s budget — but the message never arrives at the destination. The next step fails with "AssertionError: no message received from <source queue url> within 30.0s" after repeated ReceiveMessage(MaxNumberOfMessages=1, WaitTimeSeconds=3) calls. The status machine advances on a timer rather than on work done, so COMPLETED is reported over an unperformed move and the DLQ is never drained.

Why: COMPLETED is the only signal a client has that a DLQ replay finished. A runbook that dead-letters poison messages, fixes the consumer, calls StartMessageMoveTask and waits for COMPLETED concludes the backlog was replayed when nothing moved — the messages are still on the DLQ, the destination consumer sees nothing, and the operator has no error to react to. This is strictly worse than a task that fails, because the lie is indistinguishable from success.

Fix: the StartMessageMoveTask / ListMessageMoveTasks handlers and the move-task state in the SQS crate, which must actually transfer messages from the task's SourceArn queue to its DestinationArn queue (defaulting to the source's configured redrive source when DestinationArn is absent), accumulate ApproximateNumberOfMessagesMoved as it goes, and only report COMPLETED once the source of the move holds no more messages.

Related but NOT covering this cause: open issue 20261027T135029Z-sqs-startmessagemovetask-never-completes-a-dlq-drain-no.md describes a task that never reaches COMPLETED at all because its automatic-redrive precondition never fires. Here the precondition DID fire (the message reached the DLQ) and the task DID reach COMPLETED — the divergence is a COMPLETED task over a move that was never performed, which is not among that issue's causes and it does not carry this check's replay key.

Traces: sqs.message-move-task-redrives-dlq-back-to-source

=== Cause 4 of 4: StartMessageMoveTask accepts DestinationArn equal to SourceArn ===
Failing check: sqs.start-message-move-task-self-referential-destination-rejected

AWS: Smithy — StartMessageMoveTask models SourceArn (required) and DestinationArn as plain Strings with no modelled relationship, so the cross-member rule is recorded behavior only: AWS refuses a DestinationArn equal to the SourceArn before any task exists, with error code InvalidParameterValue, HTTP 400, and the message "Source queue arn and destination queue arn cannot be the same." That message is entirely fixed text — it interpolates no name, ARN, id or limit — and the refused call must leave no task behind, so a following ListMessageMoveTasks(SourceArn=<same arn>) returns an empty Results list. The identical SourceArn with a genuinely different DestinationArn is accepted and returns a TaskHandle.

Emulator: the self-referential call succeeds. The replay records HTTP 200 with a TaskHandle ('7be82683-7082-422a-af6c-69f542f813d7' in that run; the handle value itself is per-run and not part of the requirement) instead of a ClientError, so a task is created that would move a queue's messages onto itself.

Why: a client that guards against the degenerate self-move by catching InvalidParameterValue never sees it, and instead gets a live task whose semantics are undefined — in the worst case an endless self-feeding move over the same queue. The missing refusal also means ListMessageMoveTasks is polluted by a task AWS would never have created, so the "only one active task per queue" rule blocks the real move the caller goes on to request.

Fix: the input validation of the StartMessageMoveTask handler in the SQS crate, which must compare DestinationArn to SourceArn before any task state is allocated and refuse with InvalidParameterValue / 400 and the exact fixed message above.

Traces: sqs.start-message-move-task-self-referential-destination-rejected

## Evidence
Cause 1 — sqs.deploy-cfn-fifo-dlq-redrive-allow-policy-admits-only-the-stack-source
Expected (AWS): SetQueueAttributes(QueueUrl=<outsider .fifo queue>, Attributes={RedrivePolicy: {"deadLetterTargetArn":"<stack dlq arn>","maxReceiveCount":1}}) is refused with Code='InvalidParameterValue', HTTP 400 and Message='Value RedrivePolicy(maxReceiveCount=1, deadLetterTargetArn=<stack dlq arn>) for parameter RedrivePolicy is invalid. Reason: The value for the dead-letter queue's RedriveAllowPolicy attribute prevents you from completing this request.'; a subsequent GetQueueAttributes(All) on the outsider has no RedrivePolicy key.
Actual (emulator): the call succeeded — "AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', 'content-length': '2', ...}, 'RetryAttempts': 0}}" — and the refused RedrivePolicy is stored on the outsider queue.

Cause 2 — sqs.redrive-allow-policy-denyall-blocks-automatic-redrive-on-already-wired-source
Expected (AWS): with the DLQ at RedriveAllowPolicy={"redrivePermission":"denyAll"} and the source wired at maxReceiveCount=1, four successive ReceiveMessage(VisibilityTimeout=1, WaitTimeSeconds=5) calls on the SOURCE each return the same body; a direct ReceiveMessage on the DLQ returns []. After restoring {"redrivePermission":"byQueue","sourceQueueArns":[<src arn>]}, one further over-count receive redirects it and the body appears on the DLQ.
Actual (emulator): "AssertionError: expected '<token>-stuck' still delivered from the SOURCE (never redirected), got []" — the message left the source queue while denyAll was in force, so the redrive step never consulted the DLQ's RedriveAllowPolicy.

Cause 3 — sqs.message-move-task-redrives-dlq-back-to-source
Expected (AWS): after StartMessageMoveTask(SourceArn=<dlq arn>) and ListMessageMoveTasks settling on Status='COMPLETED', the body is receivable on the original source queue and GetQueueAttributes(QueueUrl=<dlq>, ['ApproximateNumberOfMessages']) reads '0'.
Actual (emulator): the task reached 'COMPLETED', then "AssertionError: no message received from https://<host>/000000000000/sim2real-sqs-mmt-src-<id> within 30.0s" (host/port/queue-name are per-run values). Nothing was moved; COMPLETED was reported over an unperformed move.

Cause 4 — sqs.start-message-move-task-self-referential-destination-rejected
Expected (AWS): StartMessageMoveTask(SourceArn=<dlq arn>, DestinationArn=<dlq arn>) raises Code='InvalidParameterValue', HTTP 400, Message='Source queue arn and destination queue arn cannot be the same.'; ListMessageMoveTasks(SourceArn=<dlq arn>) then returns Results=[].
Actual (emulator): "AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'TaskHandle': '<uuid>', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}" — accepted with a TaskHandle (the uuid is a per-run value).

## Steps to Reproduce
Cause 1:
1. CreateQueue a FIFO DLQ (name ending .fifo, Attributes {FifoQueue: 'true'}) and a FIFO source queue; read both QueueArns via GetQueueAttributes(['QueueArn']).
2. SetQueueAttributes on the DLQ with RedriveAllowPolicy = {"redrivePermission":"byQueue","sourceQueueArns":["<source arn>"]}.
3. CreateQueue a third FIFO queue (the outsider, not listed in step 2).
4. SetQueueAttributes(QueueUrl=<outsider>, Attributes={RedrivePolicy: '{"deadLetterTargetArn":"<dlq arn>","maxReceiveCount":1}'}). Observed: HTTP 200. AWS: InvalidParameterValue / 400 with the RedriveAllowPolicy Reason message.
5. GetQueueAttributes(QueueUrl=<outsider>, ['All']). Observed: RedrivePolicy present. AWS: absent.
6. Re-replay sqs.deploy-cfn-fifo-dlq-redrive-allow-policy-admits-only-the-stack-source.

Cause 2:
1. CreateQueue a DLQ and a source queue; read both QueueArns.
2. SetQueueAttributes on the DLQ with RedriveAllowPolicy = {"redrivePermission":"byQueue","sourceQueueArns":["<source arn>"]}, and on the source with RedrivePolicy = {"deadLetterTargetArn":"<dlq arn>","maxReceiveCount":1}; poll GetQueueAttributes until both read back.
3. SetQueueAttributes on the DLQ with RedriveAllowPolicy = {"redrivePermission":"denyAll"}; poll until it reads back.
4. SendMessage one body to the source queue.
5. Call ReceiveMessage(QueueUrl=<source>, MaxNumberOfMessages=1, WaitTimeSeconds=5, VisibilityTimeout=1) four times. Observed: the third call returns []. AWS: all four return the same body.
6. ReceiveMessage(QueueUrl=<dlq>, WaitTimeSeconds=2). AWS: []. 
7. Re-replay sqs.redrive-allow-policy-denyall-blocks-automatic-redrive-on-already-wired-source.

Cause 3:
1. CreateQueue a DLQ; read its QueueArn.
2. CreateQueue a source queue with Attributes {VisibilityTimeout: '2', RedrivePolicy: '{"deadLetterTargetArn":"<dlq arn>","maxReceiveCount":1}'}.
3. SendMessage one body; ReceiveMessage once, then keep calling ReceiveMessage(WaitTimeSeconds=2) past the visibility timeout until the message is redriven; confirm it is receivable on the DLQ and leave it undeleted there.
4. StartMessageMoveTask(SourceArn=<dlq arn>) -> TaskHandle.
5. Poll ListMessageMoveTasks(SourceArn=<dlq arn>, MaxResults=1) until Status='COMPLETED'.
6. ReceiveMessage(QueueUrl=<source>, WaitTimeSeconds=3) for up to 30 s. Observed: nothing. AWS: the body is back on the source queue.
7. GetQueueAttributes(QueueUrl=<dlq>, ['ApproximateNumberOfMessages']). AWS: '0'.
8. Re-replay sqs.message-move-task-redrives-dlq-back-to-source.

Cause 4:
1. CreateQueue a DLQ; read its QueueArn.
2. StartMessageMoveTask(SourceArn=<dlq arn>, DestinationArn=<dlq arn>). Observed: HTTP 200 with a TaskHandle. AWS: InvalidParameterValue / 400, Message 'Source queue arn and destination queue arn cannot be the same.'
3. ListMessageMoveTasks(SourceArn=<dlq arn>). AWS: Results=[] (the refused call leaves no task).
4. StartMessageMoveTask(SourceArn=<dlq arn>, DestinationArn=<a different queue arn>) -> accepted with a TaskHandle (positive control).
5. Re-replay sqs.start-message-move-task-self-referential-destination-rejected.
