Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__pipes__sqs.cli-create-pipe-sqs-to-lambda-workflow lambda__pipes__sqs.cli-delete-pipe-stops-delivery-negative lambda__pipes__sqs.cli-update-pipe-target-lambda-arn-workflow lambda__pipes__sqs.deploy-pipes-filter-pattern-config-positive lambda__pipes__sqs.deploy-pipes-missing-role-negative lambda__pipes__sqs.deploy-pipes-sqs-lambda-stack-positive lambda__pipes__sqs.events-pipes-enrichment-computed-field-shapes-target-via-input-template lambda__pipes__sqs.pipes-batch-size-controls-lambda-invocation-count lambda__pipes__sqs.pipes-enrichment-transforms-payload-before-target lambda__pipes__sqs.pipes-filter-criteria-drops-nonmatching-message lambda__pipes__sqs.pipes-missing-source-role-permission-negative lambda__pipes__sqs.pipes-missing-target-invoke-permission-negative lambda__pipes__sqs.pipes-sqs-fifo-source-lambda-target-ordering-positive lambda__pipes__sqs.pipes-sqs-source-lambda-target-delivery-positive lambda__pipes__sqs.pipes-sqs-visibility-timeout-respected-during-processing lambda__pipes__sqs.deploy-esm-missing-permission-cfn-negative lambda__pipes__sqs.deploy-esm-sqs-lambda-cfn-positive lambda__pipes__sqs.sqs-esm-batch-window-controls-invocation-timing lambda__pipes__sqs.sqs-esm-disabled-mapping-no-invocation-negative lambda__pipes__sqs.sqs-esm-fifo-queue-message-group-ordering-preserved lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations lambda__pipes__sqs.sqs-esm-wrong-queue-arn-no-invocation-negative lambda__pipes__sqs.sqs-event-source-mapping-invokes-lambda-positive lambda__pipes__sqs.pipes-execution-role-wrong-trust-negative
Issue-Key: lambda__pipes__sqs.cli-create-pipe-sqs-to-lambda-workflow|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: lambda__pipes__sqs.cli-delete-pipe-stops-delivery-negative|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: lambda__pipes__sqs.cli-update-pipe-target-lambda-arn-workflow|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: lambda__pipes__sqs.deploy-pipes-filter-pattern-config-positive|run#11|Timeout|resp = sqs.receive_message(QueueUrl=result_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__pipes__sqs.deploy-pipes-missing-role-negative|_drain_for_marker#7|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__pipes__sqs.deploy-pipes-sqs-lambda-stack-positive|_drain_for_marker#8|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__pipes__sqs.events-pipes-enrichment-computed-field-shapes-target-via-input-template|_drain_until#12|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__pipes__sqs.pipes-batch-size-controls-lambda-invocation-count|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: lambda__pipes__sqs.pipes-enrichment-transforms-payload-before-target|_wait_for_messages#11|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__pipes__sqs.pipes-filter-criteria-drops-nonmatching-message|_poll_accumulate#15|Timeout|time.sleep(interval_s)
Issue-Key: lambda__pipes__sqs.pipes-missing-source-role-permission-negative|_poll_pipe_state#12|Timeout|time.sleep(interval_s)
Issue-Key: lambda__pipes__sqs.pipes-missing-target-invoke-permission-negative|_poll_until#30|Timeout|time.sleep(interval_s)
Issue-Key: lambda__pipes__sqs.pipes-sqs-fifo-source-lambda-target-ordering-positive|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: lambda__pipes__sqs.pipes-sqs-source-lambda-target-delivery-positive|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: lambda__pipes__sqs.pipes-sqs-visibility-timeout-respected-during-processing|run#15|AssertionError|assert in_flight["ApproximateNumberOfMessagesNotVisible"] != "0", "pipe never picked up the message"
Issue-Key: lambda__pipes__sqs.deploy-esm-missing-permission-cfn-negative|_drain_for_marker#7|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__pipes__sqs.deploy-esm-sqs-lambda-cfn-positive|_drain_for_marker#7|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__pipes__sqs.sqs-esm-batch-window-controls-invocation-timing|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: lambda__pipes__sqs.sqs-esm-disabled-mapping-no-invocation-negative|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: lambda__pipes__sqs.sqs-esm-fifo-queue-message-group-ordering-preserved|_collect_forwarded#12|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations|run#14|Timeout|time.sleep(1)
Issue-Key: lambda__pipes__sqs.sqs-esm-wrong-queue-arn-no-invocation-negative|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: lambda__pipes__sqs.sqs-event-source-mapping-invokes-lambda-positive|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: lambda__pipes__sqs.pipes-execution-role-wrong-trust-negative|run#21|AssertionError|assert bad_final_state == "CREATE_FAILED", f"expected CREATE_FAILED, got {bad_final_state}"
Issue-Commit: 6712e9b9ed86b1379f14acf29b1637f3b8f9994b

# lambda__pipes__sqs: 3 net-new causes — a RUNNING pipe consumes nothing and invokes no target, SQS event source mappings never invoke, and CreatePipe accepts a role with the wrong trust policy

> _Note: this issue was identified against the code base at commit `6712e9b9ed86b1379f14acf29b1637f3b8f9994b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipe

## Description
Three net-new root causes. Related open issues that do NOT carry these replay keys: 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md, 20261027T020619Z-lambda-pipes-sqs-a-started-pipe-with-an.md, 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md.

Cause 1 of 3: a pipe in state RUNNING never consumes from its SQS source and never invokes its target or enrichment.
AWS: recorded behavior only for the runtime; the model fixes the surface (pipes CreatePipe takes Source, Target, RoleArn, optional Enrichment, SourceParameters.FilterCriteria and SourceParameters.SqsQueueParameters.BatchSize, and StartPipe moves CurrentState to RUNNING). In AWS a RUNNING pipe polls the source queue, applies FilterCriteria, calls the enrichment, renders InputTemplate, invokes the target and deletes the consumed messages; stopping or deleting the pipe stops consumption; messages in flight are hidden for the source queue's visibility timeout.
Emulator: the control plane reports RUNNING but nothing is ever consumed and the target is never invoked, so every data-plane check waits for the effect and is killed at the 60s harness budget (and pipes-sqs-visibility-timeout-respected-during-processing fails outright with "pipe never picked up the message").
Why: EventBridge Pipes is inert while reporting healthy — filtering, batching, enrichment, input templates, ordering and the negative permission cases are all unobservable, so a pipe-based integration cannot be built or regression-tested.
Fix: the pipes runtime behind StartPipe — a source poller that applies filter/enrichment/input-template and invokes the target, honouring the source queue's visibility timeout and stopping on StopPipe/DeletePipe.
Traces: lambda__pipes__sqs.cli-create-pipe-sqs-to-lambda-workflow, lambda__pipes__sqs.cli-delete-pipe-stops-delivery-negative, lambda__pipes__sqs.cli-update-pipe-target-lambda-arn-workflow, lambda__pipes__sqs.deploy-pipes-filter-pattern-config-positive, lambda__pipes__sqs.deploy-pipes-missing-role-negative, lambda__pipes__sqs.deploy-pipes-sqs-lambda-stack-positive, lambda__pipes__sqs.events-pipes-enrichment-computed-field-shapes-target-via-input-template, lambda__pipes__sqs.pipes-batch-size-controls-lambda-invocation-count, lambda__pipes__sqs.pipes-enrichment-transforms-payload-before-target, lambda__pipes__sqs.pipes-filter-criteria-drops-nonmatching-message, lambda__pipes__sqs.pipes-missing-source-role-permission-negative, lambda__pipes__sqs.pipes-missing-target-invoke-permission-negative, lambda__pipes__sqs.pipes-sqs-fifo-source-lambda-target-ordering-positive, lambda__pipes__sqs.pipes-sqs-source-lambda-target-delivery-positive, lambda__pipes__sqs.pipes-sqs-visibility-timeout-respected-during-processing

Cause 2 of 3: an SQS event source mapping never produces an invocation.
AWS: recorded behavior only for the poller; the model fixes the surface (lambda CreateEventSourceMapping with EventSourceArn/FunctionName/BatchSize/MaximumBatchingWindowInSeconds/ScalingConfig and State). AWS polls the source and invokes the function with an SQS Records envelope, honouring batch size, batching window, Enabled=false and maximum concurrency.
Emulator: no invocation ever occurs, so each ESM check times out at 60s.
Why: the queue -> function topology is inert while CreateEventSourceMapping reports success, so neither the positive deliveries nor the negative "should not be invoked" cases carry any information.
Fix: the lambda event-source-mapping poller/runtime.
Traces: lambda__pipes__sqs.deploy-esm-missing-permission-cfn-negative, lambda__pipes__sqs.deploy-esm-sqs-lambda-cfn-positive, lambda__pipes__sqs.sqs-esm-batch-window-controls-invocation-timing, lambda__pipes__sqs.sqs-esm-disabled-mapping-no-invocation-negative, lambda__pipes__sqs.sqs-esm-fifo-queue-message-group-ordering-preserved, lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations, lambda__pipes__sqs.sqs-esm-wrong-queue-arn-no-invocation-negative, lambda__pipes__sqs.sqs-event-source-mapping-invokes-lambda-positive

Cause 3 of 3: CreatePipe accepts a RoleArn whose trust policy does not permit pipes.amazonaws.com, and the pipe reaches RUNNING instead of CREATE_FAILED.
AWS: recorded behavior only; the model gives pipes CreatePipe the error `ValidationException` and the RequestedPipeState/CurrentState pair. AWS validates that the pipe can assume RoleArn; a role that does not trust pipes.amazonaws.com drives the pipe to CurrentState=CREATE_FAILED with a StateReason naming the assume-role failure (the role ARN is the variable part).
Emulator: `expected CREATE_FAILED, got RUNNING` — the trust policy is never evaluated.
Why: a pipe that cannot assume its role is reported healthy, so an operator has no signal that the integration is dead and the usual IaC guardrail against a mis-trusted role is gone.
Fix: the pipes CreatePipe handler's role validation and the pipe state machine's CREATE_FAILED path.
Traces: lambda__pipes__sqs.pipes-execution-role-wrong-trust-negative

## Evidence
Cause 1 — `Error: Check timed out after 60.0s` on all 14 pipe data-plane checks, plus `AssertionError: pipe never picked up the message` for pipes-sqs-visibility-timeout-respected-during-processing.
Cause 2 — `Error: Check timed out after 60.0s` on all 8 ESM checks; no invocation and no message consumption observed.
Cause 3 — `AssertionError: expected CREATE_FAILED, got RUNNING`.

## Steps to Reproduce
Cause 1: CreatePipe(Source=<queue arn>, Target=<function arn>, RoleArn=<valid role>), StartPipe, wait for CurrentState=RUNNING, SendMessage, then wait for the target invocation — none occurs and the message stays on the queue.
Cause 2: CreateEventSourceMapping(EventSourceArn=<queue arn>, FunctionName=<fn>), SendMessage, wait for an invocation — none occurs.
Cause 3: create a role whose trust policy names a service other than pipes.amazonaws.com, CreatePipe with it, then poll DescribePipe — CurrentState becomes RUNNING.
