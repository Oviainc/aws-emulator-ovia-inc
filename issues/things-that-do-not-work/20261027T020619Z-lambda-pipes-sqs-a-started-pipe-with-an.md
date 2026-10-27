Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__pipes__sqs.events-pipes-enrichment-sees-whole-batch-and-target-gets-its-single-result lambda__pipes__sqs.events-pipes-enrichment-stopped-pipe-consumes-nothing-and-start-drains-backlog
Issue-Key: lambda__pipes__sqs.events-pipes-enrichment-sees-whole-batch-and-target-gets-its-single-result|_drain_until#14|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__pipes__sqs.events-pipes-enrichment-stopped-pipe-consumes-nothing-and-start-drains-backlog|_drain_until#17|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Commit: 9a356a0438a215c6e535efb212b208d427610e62

# lambda__pipes__sqs: a started pipe with an enrichment never consumes its source or delivers to its target, so batch-aggregating enrichment and stop/start backlog draining never happen (1 cause)

> _Note: this issue was identified against the code base at commit `9a356a0438a215c6e535efb212b208d427610e62`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipe

## Description
One net-new root cause, grounded in two failing behavioral checks (Cause 1 of 1).
AWS: recorded behavior only for the data-plane timing (the Pipes model describes the control plane: CreatePipe/StartPipe/StopPipe/DescribePipe with CurrentState). On AWS, a pipe whose source is an SQS queue, whose enrichment is a Lambda and whose target is a second SQS queue, once DescribePipe reports CurrentState RUNNING, polls the source queue in batches, invokes the enrichment once per batch with the whole batch, and sends the enrichment's single returned result to the target — so the target receives one message per enrichment invocation, not one per source record. A pipe left STOPPED consumes nothing: a backlog accumulates on the source and is drained only after StartPipe.
Emulator: with the pipe reported in its desired state, ten messages sent to the source queue are never consumed and the target queue never receives any message; both checks poll their target (bounded at 200 s in the check) and are cut off by the suite's per-check deadline with `Error: Check timed out after 60.0s`. Nothing the enrichment produced is observable, and the stopped/started distinction cannot be observed either because the running pipe drains nothing. The open issue 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md records the same "a running pipe does nothing" symptom for two sibling checks in this group but does not carry these two replay keys; these two cases add the batching contract (one target message per enrichment invocation, carrying the aggregate over the whole batch) and the stop/start backlog-draining contract.
Why: a client that builds an SQS -> enrichment -> SQS pipe sees its source backlog sit untouched and its target stay empty forever; a consumer waiting on the target hangs, messages are never deleted from the source, and the per-batch aggregation semantics (one enrichment invocation per batch, one target message per invocation) can never be relied on. Stopping a pipe to park work and starting it to drain the backlog has no observable effect either.
Fix: the EventBridge Pipes runtime — the source poller that must read batches from the SQS source while CurrentState is RUNNING (and read nothing while STOPPED), invoke the enrichment once per batch with the full batch, forward the enrichment result to the target, and delete the consumed source messages.
Traces: lambda__pipes__sqs.events-pipes-enrichment-sees-whole-batch-and-target-gets-its-single-result, lambda__pipes__sqs.events-pipes-enrichment-stopped-pipe-consumes-nothing-and-start-drains-backlog.

## Evidence
Both checks fail with the same report detail:
- lambda__pipes__sqs.events-pipes-enrichment-sees-whole-batch-and-target-gets-its-single-result: `Error: Check timed out after 60.0s`
- lambda__pipes__sqs.events-pipes-enrichment-stopped-pipe-consumes-nothing-and-start-drains-backlog: `Error: Check timed out after 60.0s`

Expected: after SendMessageBatch puts a ten-record backlog on the source queue (confirmed via GetQueueAttributes ApproximateNumberOfMessages == 10) and StartPipe is called, the target queue receives aggregate messages that between them cover all ten record markers, every target message is an enrichment aggregate (carries the batch_len the enrichment computed), and the number of target messages equals the number of enrichment invocations rather than ten.
Actual: the source backlog is never consumed and no message ever arrives on the target; the check's own 200 s drain poll makes no progress and the suite's 60 s per-check deadline cuts it off. DescribePipe reports the desired CurrentState, so the control plane accepts the pipe while the data plane does nothing.

## Steps to Reproduce
Create two SQS queues (source, target) and a Lambda enrichment that returns one aggregate per invocation; create an IAM role trusted by pipes.amazonaws.com with read on the source, invoke on the enrichment and send on the target; `aws pipes create-pipe --name <p> --source <source-arn> --target <target-arn> --enrichment <fn-arn> --role-arn <role>` and wait for DescribePipe CurrentState RUNNING; SendMessageBatch ten messages to the source; poll the target with ReceiveMessage. Observe the target stays empty and the source backlog is never consumed. For the stop/start case, create the pipe in STOPPED state, send the backlog (nothing must be consumed), then StartPipe and watch for the backlog to drain. Replay: lambda__pipes__sqs.events-pipes-enrichment-sees-whole-batch-and-target-gets-its-single-result, lambda__pipes__sqs.events-pipes-enrichment-stopped-pipe-consumes-nothing-and-start-drains-backlog.
