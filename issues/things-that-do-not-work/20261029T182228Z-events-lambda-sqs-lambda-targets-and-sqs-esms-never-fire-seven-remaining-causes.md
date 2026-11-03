Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.eventbridge-event-envelope-format-to-lambda events__lambda__sqs.eventbridge-lambda-sourcearn-condition-arn-resolution events__lambda__sqs.eventbridge-multiple-rules-single-event-fanout events__lambda__sqs.cli-eventbridge-remove-targets-stops-delivery-negative events__lambda__sqs.cli-lambda-add-permission-for-eventbridge-invoke events__lambda__sqs.cli-put-rule-put-targets-lambda-sqs-fanout events__lambda__sqs.eventbridge-fanout-lambda-and-sqs events__lambda__sqs.eventbridge-lambda-target-missing-permission-dlq events__lambda__sqs.eventbridge-lambda-target-without-resource-policy-no-invoke events__lambda__sqs.eventbridge-rule-dlq-on-failed-lambda-invoke events__lambda__sqs.eventbridge-rule-input-transformer-reshapes-lambda-payload events__lambda__sqs.eventbridge-rule-sqs-lambda-nested-detail-survives-to-dynamodb events__lambda__sqs.eventbridge-schedule-expression-triggers-lambda events__lambda__sqs.eventbridge-to-lambda-to-sqs-chain events__lambda__sqs.events-delivery-and-failure-deleted-lambda-target-dead-letters-as-no-permissions events__lambda__sqs.cli-create-event-source-mapping-sqs-lambda events__lambda__sqs.cli-delete-event-source-mapping-stops-polling events__lambda__sqs.lambda-esm-dlq-on-repeated-failure events__lambda__sqs.lambda-sqs-partial-batch-failure-reporting events__lambda__sqs.sqs-esm-batch-size-controls-invocation-count events__lambda__sqs.sqs-esm-disabled-mapping-stops-delivery events__lambda__sqs.sqs-esm-partial-batch-failure-retries-only-failed events__lambda__sqs.sqs-fifo-esm-preserves-message-group-order events__lambda__sqs.sqs-lambda-event-source-mapping-basic events__lambda__sqs.sqs-to-lambda-event-source-mapping events__lambda__sqs.lambda-role-missing-sqs-permissions-blocks-poll events__lambda__sqs.lambda-without-sqs-permission-esm-creation-fails events__lambda__sqs.sqs-lambda-event-source-mapping-missing-role-permission-rejected events__lambda__sqs.sqs-esm-dlq-redrive-after-max-receive events__lambda__sqs.xsvc-lambda-1-durable-execution-timeout-emits-timed-out-status-change-to-sqs events__lambda__sqs.xsvc-lambda-1-durable-failed-status-rule-rejects-succeeded-executions events__lambda__sqs.cfn-deploy-eventbridge-rule-lambda-target
Issue-Key: events__lambda__sqs.eventbridge-event-envelope-format-to-lambda|_reemit_until_delivered#13|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sqs.eventbridge-lambda-sourcearn-condition-arn-resolution|_reemit_until_delivered#17|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sqs.eventbridge-multiple-rules-single-event-fanout|_emit_until_delivered#11|AssertionError|raise AssertionError(f"event never delivered to: {missing} after {attempts} attempts")
Issue-Key: events__lambda__sqs.cli-eventbridge-remove-targets-stops-delivery-negative|_drain_queue_for#16|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.cli-lambda-add-permission-for-eventbridge-invoke|_drain_queue_for#19|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.cli-put-rule-put-targets-lambda-sqs-fanout|_drain_queue_for#18|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.eventbridge-fanout-lambda-and-sqs|_collect_messages#15|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.eventbridge-lambda-target-missing-permission-dlq|_drain#17|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.eventbridge-lambda-target-without-resource-policy-no-invoke|_collect_messages#20|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.eventbridge-rule-dlq-on-failed-lambda-invoke|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sqs.eventbridge-rule-input-transformer-reshapes-lambda-payload|_drain_queue#7|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.eventbridge-rule-sqs-lambda-nested-detail-survives-to-dynamodb|run#38|Timeout|time.sleep(1)
Issue-Key: events__lambda__sqs.eventbridge-schedule-expression-triggers-lambda|_drain_queue#7|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.eventbridge-to-lambda-to-sqs-chain|_wait_for_messages#15|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.events-delivery-and-failure-deleted-lambda-target-dead-letters-as-no-permissions|_drain#17|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.cli-create-event-source-mapping-sqs-lambda|_drain_queue_for#12|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sqs.cli-delete-event-source-mapping-stops-polling|_drain_queue_for#12|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sqs.lambda-esm-dlq-on-repeated-failure|_wait_for_messages#11|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.lambda-sqs-partial-batch-failure-reporting|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sqs.sqs-esm-batch-size-controls-invocation-count|_drain#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.sqs-esm-disabled-mapping-stops-delivery|_drain#9|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.sqs-esm-partial-batch-failure-retries-only-failed|_drain#11|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.sqs-fifo-esm-preserves-message-group-order|_drain#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.sqs-lambda-event-source-mapping-basic|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sqs.sqs-to-lambda-event-source-mapping|_wait_for_messages#8|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.lambda-role-missing-sqs-permissions-blocks-poll|run#6|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sqs.lambda-without-sqs-permission-esm-creation-fails|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: events__lambda__sqs.sqs-lambda-event-source-mapping-missing-role-permission-rejected|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: events__lambda__sqs.sqs-esm-dlq-redrive-after-max-receive|run#18|AssertionError|assert delivery_attempts >= 2, (
Issue-Key: events__lambda__sqs.xsvc-lambda-1-durable-execution-timeout-emits-timed-out-status-change-to-sqs|_collect#10|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: events__lambda__sqs.xsvc-lambda-1-durable-failed-status-rule-rejects-succeeded-executions|_collect#10|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: events__lambda__sqs|GetPolicy|ResourceNotFoundException
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# events/lambda/sqs: EventBridge Lambda targets and SQS event source mappings never fire, ESM role validation is absent, and DLQ redrive counts wrong (7 causes)

> _These are the seven still-open causes of
> `20261029T182228Z-events-lambda-sqs-eventbridge-sqs-targets-ignore-the.md`, re-filed under
> their own name when the eighth (cause 1 of that bundle — an EventBridge SQS target ignoring
> the queue's resource policy) was fixed and its report moved to `issues/things-that-now-work/`.
> Thirty of that bundle's forty checks still fail, and they are the ones below. The cause
> numbering is kept as it was filed so the evidence lines up._
>
> _Note: the bundle was identified against the code base at commit
> `b1cd28f1ca277d6a032df605f83af5017e934043`. The code base has drifted since, so trace each
> cause back to the current code before changing anything._

## Operation
PutTargets

## Description

Seven root causes. 20261025T084539Z-events-lambda-sqs-an-eventbridge-rule-s-target.md, 20261025T002357Z-events-lambda-sqs-eventbridge-lambda-targets-and-sqs-event-source-mappings.md, 20261025T022656Z-events-lambda-sqs-a-fifo-queue-event-source.md, 20261027T191349Z-events-lambda-sqs-lambda-durable-execution-status-change-events-are-never.md and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md own other checks in this service; none of the checks below were bound to them.

**What blocks most of them.** Causes 2, 3, 4, 6 and 7 all wait on a Lambda function RUNNING its
deployment package: every one of their checks asserts on a queue that the function under test
writes to, and
`services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs` states in its own
header that "running the deployment package is NOT implemented: this emulator has no in-process
runtime for a Lambda zip archive or container image". That engine is the subject of
`20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md` and
`20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md`,
and nothing below can be observed until one of them lands.

**A related gap, already fixed for one target kind.** EventBridge now authorizes a QUEUE
target's delivery against that queue's own resource policy
(`service-com-amazonaws-eventbridge-awsevents`: `state::target_policy_admits`,
`ops::put_events::queue_delivery_refusal`). "For Lambda functions, Amazon SNS topics and Amazon
SQS queues, EventBridge relies on resource-based policies" (`PutTargets`), so the same decision
is owed by a Lambda function target and an SNS topic target — filed as
`20261029T182228Z-events-iam-lambda-sqs-an-eventbridge-rule-with-no-lambda-invokefunction-grant.md`
and `20261029T182228Z-events-lambda-sns-eventbridge-publishes-to-sns-topics-with-no-topic....md`.
`state::target_policy_admits` takes the attempt's action and resource, so those invokers reuse it
rather than restating the rule. Cause 3's negative checks (a Lambda target with no resource
policy, and the dead letter that records one) need both that gate and the execution engine.

Cause 2 of 8 (the dead-letter trace of this cause now passes; the three lambda-to-SQS chains remain): in other configurations the same SQS target never receives anything at all.
AWS: a matching event reaches every SQS target of every matching rule exactly once.
Emulator: "AssertionError: event never delivered to <queue url> after 8 attempts of 6.0s each" and "AssertionError: event never delivered to: ['<queue url>'] after 15 attempts" — the queue URL is a per-run value; the requirement is one message per matched rule. The three remaining traces are lambda-to-SQS chains: the queue they read is one the invoked FUNCTION writes to, so they wait on the execution engine rather than on the rule dispatcher.
Why: delivery is not merely unauthorized-but-permissive, it is also unreliable: the same surface both over-delivers and under-delivers, so no client can reason about it.
Fix: the EventBridge rule dispatcher's SQS-target invoker and its multi-rule fan-out loop.
Traces: events__lambda__sqs.eventbridge-event-envelope-format-to-lambda, events__lambda__sqs.eventbridge-lambda-sourcearn-condition-arn-resolution, events__lambda__sqs.eventbridge-multiple-rules-single-event-fanout

Cause 3 of 8: an EventBridge rule never invokes its Lambda target, on a pattern or on a schedule.
AWS: a matching PutEvents, or a ScheduleExpression firing, invokes each Lambda target once with the envelope (or the InputTransformer rendering); RemoveTargets stops it; a target lacking the resource-policy grant is dead-lettered with ERROR_CODE naming the permission failure.
Emulator: the function is never invoked; 12 checks end "Check timed out after 60.0s" and every paired negative (RemoveTargets, missing permission, DLQ) is ungraded.
Why: the EventBridge→Lambda pattern is unusable and no authorization or DLQ behaviour can be observed.
Fix: the EventBridge Lambda-target invoker, its InputTransformer rendering, the scheduled-rule timer, and the dead-letter path.
Traces: events__lambda__sqs.cli-eventbridge-remove-targets-stops-delivery-negative, events__lambda__sqs.cli-lambda-add-permission-for-eventbridge-invoke, events__lambda__sqs.cli-put-rule-put-targets-lambda-sqs-fanout, events__lambda__sqs.eventbridge-fanout-lambda-and-sqs, events__lambda__sqs.eventbridge-lambda-target-missing-permission-dlq, events__lambda__sqs.eventbridge-lambda-target-without-resource-policy-no-invoke, events__lambda__sqs.eventbridge-rule-dlq-on-failed-lambda-invoke, events__lambda__sqs.eventbridge-rule-input-transformer-reshapes-lambda-payload, events__lambda__sqs.eventbridge-rule-sqs-lambda-nested-detail-survives-to-dynamodb, events__lambda__sqs.eventbridge-schedule-expression-triggers-lambda, events__lambda__sqs.eventbridge-to-lambda-to-sqs-chain, events__lambda__sqs.events-delivery-and-failure-deleted-lambda-target-dead-letters-as-no-permissions, events__lambda__sqs.cfn-deploy-eventbridge-rule-lambda-target

Cause 4 of 8: an SQS event source mapping never invokes the function, so none of BatchSize, Enabled, partial-batch reporting, DLQ-on-repeated-failure or FIFO MessageGroupId ordering has any effect.
AWS: an enabled ESM polls the queue and invokes the function with up to BatchSize records; a disabled mapping stops polling; a ReportBatchItemFailures response retries only the listed itemIdentifiers; a FIFO source preserves per-MessageGroupId order.
Emulator: the function is never invoked; 10 checks end "Check timed out after 60.0s".
Why: SQS-driven Lambda processing does nothing while every configuration knob reports as accepted.
Fix: the Lambda event-source-mapping poller for SQS (batching, enable/disable, partial-batch failure handling, FIFO group ordering).
Traces: events__lambda__sqs.cli-create-event-source-mapping-sqs-lambda, events__lambda__sqs.cli-delete-event-source-mapping-stops-polling, events__lambda__sqs.lambda-esm-dlq-on-repeated-failure, events__lambda__sqs.lambda-sqs-partial-batch-failure-reporting, events__lambda__sqs.sqs-esm-batch-size-controls-invocation-count, events__lambda__sqs.sqs-esm-disabled-mapping-stops-delivery, events__lambda__sqs.sqs-esm-partial-batch-failure-retries-only-failed, events__lambda__sqs.sqs-fifo-esm-preserves-message-group-order, events__lambda__sqs.sqs-lambda-event-source-mapping-basic, events__lambda__sqs.sqs-to-lambda-event-source-mapping

Cause 5 of 8: CreateEventSourceMapping accepts an execution role with no SQS permissions.
AWS: the call is refused with HTTP 400 InvalidParameterValueException, message of the fixed form "The provided execution role does not have permissions to call ReceiveMessage on SQS".
Emulator: the call returns HTTP 202 with a UUID, BatchSize, EventSourceArn, FunctionArn, State="Creating" and an EventSourceMappingArn. "AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {... 'State': 'Creating' ...}" — the UUID/ARNs are per-run values; the requirement is the 400 and that message.
Why: the one validation AWS performs at create time is skipped, so a mapping that can never poll is created and the caller is told it succeeded.
Fix: execution-role permission validation in the Lambda CreateEventSourceMapping handler. NOTE for the fixer: the handler already asks — `create_event_source_mapping.rs` calls `state::unreadable_event_source`, and `state::event_source_authorization` lists the queue reads (`ReceiveMessage`, `DeleteMessage`, `GetQueueAttributes`) and the sentence. The recorded roles get past it through that module's documented "shows nothing" escape: a role carrying an attachment whose document this account's IAM store does not hold (an AWS-managed policy) is not refused, because refusing on evidence the emulator does not have would deny access AWS permits. Deciding whether that escape should hold for an event source is a decision about every service that resolves a caller-supplied role, and the refusal's wording has to be settled against the recording too (the emulator words it "The function execution role does not have permissions to call ReceiveMessage on SQS").
Traces: events__lambda__sqs.lambda-role-missing-sqs-permissions-blocks-poll, events__lambda__sqs.lambda-without-sqs-permission-esm-creation-fails, events__lambda__sqs.sqs-lambda-event-source-mapping-missing-role-permission-rejected

Cause 6 of 8: a queue's redrive policy moves a message to the DLQ after zero delivery attempts.
AWS: with a RedrivePolicy of maxReceiveCount=N, a message is moved to the dead-letter queue only after it has been received N times; with N=2 it must survive at least the first receive.
Emulator: the message appears in the DLQ having never been delivered. "AssertionError: message reached the DLQ after 0 delivery attempt(s); maxReceiveCount=2 requires it to survive at least the first". maxReceiveCount is a value from the request; the rule — move only after the count is exceeded — is the requirement.
Why: every message is dead-lettered immediately instead of being processed, so a client's retry budget is nullified and healthy traffic lands in the DLQ.
Fix: the SQS receive path's receive-count accounting and redrive evaluation. NOTE for the fixer: "0 delivery attempt(s)" is counted by the scenario's own function, which never runs (cause 4); the SQS store itself already moves a message only once its per-queue receive count has reached maxReceiveCount, so check this cause again once the ESM poller delivers.
Traces: events__lambda__sqs.sqs-esm-dlq-redrive-after-max-receive

Cause 7 of 8: Lambda durable-execution status-change events are never published to EventBridge.
AWS: a durable execution emits on the default bus source "aws.lambda" with a detail-type naming the status change, detail carrying the execution identifier and a status of SUCCEEDED/FAILED/TIMED_OUT/STOPPED; a rule scoped to one status receives only that status.
Emulator: the rule's SQS target stays empty for every status; both checks end "Check timed out after 60.0s".
Why: durable-execution orchestration cannot react to completion or failure at all, and the status filter cannot be validated.
Fix: the Lambda durable-execution state machine must publish its status-change events to the EventBridge bus.
Traces: events__lambda__sqs.xsvc-lambda-1-durable-execution-timeout-emits-timed-out-status-change-to-sqs, events__lambda__sqs.xsvc-lambda-1-durable-failed-status-rule-rejects-succeeded-executions

Cause 8 of 8: a Lambda function created by CloudFormation is invisible to the Lambda data plane.
AWS: after the stack reaches CREATE_COMPLETE, GetPolicy/GetFunction/Invoke on the stack-generated physical id succeed.
Emulator: GetPolicy answers HTTP 404 ResourceNotFoundException "Function not found: <function ARN>" — "Function not found: " is fixed text, the ARN is the stack's generated physical id. The same cause is filed for other services as 20261023T004328Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md; this check was not bound to it.
Why: every IaC-deployed function is unreachable by SDK, so the whole deploy-then-wire workflow breaks at the first call.
Fix: the CloudFormation AWS::Lambda::Function provider must register the function in the Lambda crate's state under the physical id it returns.
Traces: events__lambda__sqs.cfn-deploy-eventbridge-rule-lambda-target

## Evidence
Cause 2: "AssertionError: event never delivered to https://127.0.0.1:<port>/<account>/<queue> after 8 attempts of 6.0s each"; "AssertionError: event never delivered to: [...] after 15 attempts".
Causes 3, 4 and 7: 25 checks end "Error: Check timed out after 60.0s".
Cause 5: "AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'UUID': ..., 'State': 'Creating', ...}" — expected HTTP 400 InvalidParameterValueException naming the missing ReceiveMessage permission.
Cause 6: "AssertionError: message reached the DLQ after 0 delivery attempt(s); maxReceiveCount=2 requires it to survive at least the first".
Cause 8: "botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetPolicy operation: Function not found: arn:aws:lambda:<region>:<account>:function:<stack>-ForwarderFunction-<suffix>".

## Steps to Reproduce
(1) PutRule with a Lambda target plus AddPermission, and a scheduled rule — never invoked. (2) Repeat cause 2's chains: PutRule with a Lambda target whose function forwards to a queue, PutEvents a matching entry, read that queue — the message never arrives. (3) CreateEventSourceMapping on a standard and a FIFO queue, SendMessage — never invoked. (4) CreateEventSourceMapping with a role lacking sqs:ReceiveMessage — 202. (5) SetQueueAttributes with RedrivePolicy maxReceiveCount=2, SendMessage, let an ESM poll it — the message reaches the DLQ with no delivery. (6) Run a durable execution to TIMED_OUT and poll a rule's queue — empty. (7) CreateStack with AWS::Lambda::Function, then GetPolicy on the physical id — 404.
