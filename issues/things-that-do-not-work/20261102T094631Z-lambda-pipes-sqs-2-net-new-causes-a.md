Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__pipes__sqs.cli-create-pipe-sqs-to-lambda-workflow lambda__pipes__sqs.cli-delete-pipe-stops-delivery-negative lambda__pipes__sqs.pipes-batch-size-controls-lambda-invocation-count lambda__pipes__sqs.pipes-filter-criteria-drops-nonmatching-message lambda__pipes__sqs.pipes-sqs-fifo-source-lambda-target-ordering-positive lambda__pipes__sqs.pipes-sqs-source-lambda-target-delivery-positive lambda__pipes__sqs.sqs-esm-wrong-queue-arn-no-invocation-negative
Issue-Key: lambda__pipes__sqs.cli-create-pipe-sqs-to-lambda-workflow|_drain_json_messages#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=max_messages, WaitTimeSeconds=wait_s)
Issue-Key: lambda__pipes__sqs.cli-delete-pipe-stops-delivery-negative|_drain_json_messages#13|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=max_messages, WaitTimeSeconds=wait_s)
Issue-Key: lambda__pipes__sqs.pipes-batch-size-controls-lambda-invocation-count|_drain_json_messages#14|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=max_messages, WaitTimeSeconds=wait_s)
Issue-Key: lambda__pipes__sqs.pipes-filter-criteria-drops-nonmatching-message|_drain_json_messages#15|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=max_messages, WaitTimeSeconds=wait_s)
Issue-Key: lambda__pipes__sqs.pipes-sqs-fifo-source-lambda-target-ordering-positive|_drain_json_messages#16|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=max_messages, WaitTimeSeconds=wait_s)
Issue-Key: lambda__pipes__sqs.pipes-sqs-source-lambda-target-delivery-positive|_drain_json_messages#11|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=max_messages, WaitTimeSeconds=wait_s)
Issue-Key: lambda__pipes__sqs.sqs-esm-wrong-queue-arn-no-invocation-negative|_drain_json_messages#19|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=max_messages, WaitTimeSeconds=wait_s)
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# lambda__pipes__sqs: 2 net-new causes — a RUNNING pipe with an SQS source never delivers to its Lambda target, and an SQS event source mapping never invokes, so the negative legs have no control

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Two net-new root causes; six and one failing checks respectively, all of which exhaust the 60 s harness budget.

Cause 1 of 2: a pipe whose source is an SQS queue and whose target is a Lambda function never moves a message.
AWS: once CreatePipe/StartPipe leaves the pipe in CurrentState RUNNING, the pipe polls the source queue, applies FilterCriteria (non-matching records are dropped and deleted), batches according to SourceParameters.SqsQueueParameters.BatchSize, invokes the target once per batch, and preserves MessageGroupId ordering for a FIFO source; DeletePipe stops the delivery. Smithy (pipes CreatePipe/StartPipe/DescribePipe -> RequestedPipeState/PipeState) models the configuration and states but not the data movement — delivery is asynchronous, so this is recorded behavior only.
Emulator: every one of these checks exhausts the 60 s budget with no invocation and no message consumption ('Error: Check timed out after 60.0s'). The negative legs (delete-pipe stops delivery, filter drops non-matching) have no positive control, so they are unprovable rather than passing.
Why: a client that wires SQS to Lambda through EventBridge Pipes sees a pipe that reports RUNNING and transports nothing; batch size, filtering and FIFO ordering cannot be validated at all.
Fix: the pipe runtime for SQS sources — poller, filter evaluation, batching and target invocation — plus honouring DeletePipe/StopPipe in that runtime.
Traces: lambda__pipes__sqs.cli-create-pipe-sqs-to-lambda-workflow, lambda__pipes__sqs.cli-delete-pipe-stops-delivery-negative, lambda__pipes__sqs.pipes-batch-size-controls-lambda-invocation-count, lambda__pipes__sqs.pipes-filter-criteria-drops-nonmatching-message, lambda__pipes__sqs.pipes-sqs-fifo-source-lambda-target-ordering-positive, lambda__pipes__sqs.pipes-sqs-source-lambda-target-delivery-positive.

Cause 2 of 2: an enabled SQS event source mapping never invokes its function, so the wrong-queue-ARN negative cannot be distinguished from the mapping not working.
AWS: an enabled mapping on queue A invokes the function with A's records; a mapping created against a different queue's ARN never invokes for messages sent to A. The negative claim is only meaningful next to the working control. Smithy (lambda CreateEventSourceMapping) models the configuration, not the polling — recorded behavior only.
Emulator: the control mapping delivers nothing inside the 60 s budget, so the check times out.
Why: clients cannot tell a mis-wired EventSourceArn from an event source mapping subsystem that never polls.
Fix: the event-source-mapping poller/invoker for SQS sources.
Traces: lambda__pipes__sqs.sqs-esm-wrong-queue-arn-no-invocation-negative. Related open issue that does NOT carry these replay keys: 20261029T193114Z-lambda-pipes-sqs-3-net-new-causes-a.md.

## Evidence
Cause 1 — Expected: the target function is invoked with the source records (once per batch per BatchSize, in MessageGroupId order for FIFO), non-matching records are dropped, and delivery stops after DeletePipe. Actual: 'Error: Check timed out after 60.0s' for all six checks; no invocation observed.
Cause 2 — Expected: the control mapping on the correct queue ARN invokes the function, while the mapping on the other ARN does not. Actual: 'Error: Check timed out after 60.0s' — the control never invokes.

## Steps to Reproduce
Cause 1: CreateQueue (standard and FIFO), CreateFunction, CreatePipe with the queue as Source, the function as Target, a role granting both, optional FilterCriteria and SqsQueueParameters.BatchSize; wait for CurrentState RUNNING, SendMessage, and poll for invocations; then DeletePipe and send again. Cause 2: CreateEventSourceMapping with the correct queue ARN (control) and one with another queue's ARN; SendMessage to the first queue and poll for invocations. Replay the seven check ids in trace_ids.
