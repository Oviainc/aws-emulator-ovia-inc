Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.cfn-deploy-lambda-destination-sns-on-failure lambda__sns__sqs.sqs-lambda-esm-maximum-concurrency-limits
Issue-Key: lambda__sns__sqs.cfn-deploy-lambda-destination-sns-on-failure|_poll_until#14|Timeout|last = probe()
Issue-Key: lambda__sns__sqs.sqs-lambda-esm-maximum-concurrency-limits|_poll_until#16|Timeout|last = probe()
Issue-Commit: 145c20a94e0bd5c99471752c6f247be888f6ff73

# lambda__sns__sqs: 2 net-new causes — a CFN-deployed Lambda OnFailure destination to an SNS topic never publishes the invocation record, and an SQS ESM never honours ScalingConfig.MaximumConcurrency

> _Note: this issue was identified against the code base at commit `145c20a94e0bd5c99471752c6f247be888f6ff73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFunctionEventInvokeConfig

## Description
Two root causes in this scenario service.

Cause 1 of 2: an async OnFailure destination pointing at an SNS topic never publishes the invocation record.
AWS: recorded behavior only for the delivery (Lambda's Smithy model declares `DestinationConfig.OnFailure.Destination`, which may be an SNS topic ARN, but states no runtime rule). The recording shows that when a forced, always-failing asynchronous invocation exhausts its retries, Lambda publishes an invocation-record notification to the destination topic — a JSON document naming the function, the request id, the condition `RetriesExhausted`, the original request payload and the real error — which the topic's SQS subscriber then receives.
Emulator: nothing is ever published to the topic, so nothing reaches the observing queue; the check exhausts its budget and reports `Check timed out after 60.0s`.
Why: the SNS fan-out of Lambda failure records is silent, so every failed async event is lost rather than announced; alerting subscribed to the destination topic never fires.
Fix: the Lambda asynchronous invocation/retry driver and its destination dispatch for an SNS destination (the lambda service's async invocation component), reached here via CloudFormation's AWS::Lambda::EventInvokeConfig. Related but not covering: 20261107T014900Z-lambda-sns-sqs-async-destinations-and-dead-letter.md owns other checks in this service; this check was not marked owned by it in this run.
Traces: lambda__sns__sqs.cfn-deploy-lambda-destination-sns-on-failure.

Cause 2 of 2: `ScalingConfig.MaximumConcurrency` on an SQS event source mapping is never exercised — the backlog is never drained concurrently.
AWS: recorded behavior only for the scaling (Lambda's Smithy model declares `CreateEventSourceMapping`'s `ScalingConfig.MaximumConcurrency`, @range 2-1000, but no runtime rule). The recording shows that with a backlog large enough to scale higher, the ESM runs at most `MaximumConcurrency` Lambda invocations at once and still drains the queue, which the check measures through an independent side-channel queue recording each invocation's start and end.
Emulator: the check never observes the invocations it needs and exhausts its 60s budget, so the cap is neither honoured nor measurable — the ESM does not drive the concurrent invocations at all.
Why: a client relying on MaximumConcurrency to protect a downstream dependency cannot verify the cap, and more importantly the queue is not drained as AWS drains it; throughput and ordering behavior of the ESM are unobservable.
Fix: the Lambda event-source-mapping poller — its concurrency scaler and the ScalingConfig.MaximumConcurrency ceiling it must apply (the lambda service's ESM runtime component).
Traces: lambda__sns__sqs.sqs-lambda-esm-maximum-concurrency-limits.

## Evidence
Cause 1 — cfn-deploy-lambda-destination-sns-on-failure: 'Check timed out after 60.0s'; the check deploys a role, topic, observing queue + subscription and function with an async OnFailure destination set to the topic, forces an always-failing async invoke, and polls the queue for the invocation-record notification — none arrives.
Cause 2 — sqs-lambda-esm-maximum-concurrency-limits: 'Check timed out after 60.0s'; the check creates src/out queues and an SQS ESM with ScalingConfig.MaximumConcurrency, floods the source, and measures overlap from per-invocation start/end records on the out queue — the records never arrive.

## Steps to Reproduce
1. CreateStack (or create by API) a Lambda function whose code always raises, an execution role allowing sns:Publish, an SNS topic with an SQS subscriber, and an EventInvokeConfig with DestinationConfig.OnFailure = the topic ARN.
2. Invoke asynchronously with retries exhausted; poll the subscriber queue — no invocation-record notification arrives.
3. Create a source queue and an SQS event source mapping with ScalingConfig.MaximumConcurrency set, send a deep backlog, and have the handler record start/end timestamps to a second queue: no records arrive, so the backlog is not drained concurrently and the cap cannot be observed.
