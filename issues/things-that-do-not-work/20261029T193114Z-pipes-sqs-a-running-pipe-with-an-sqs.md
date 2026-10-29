Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes__sqs.pipe-filter-criteria-non-matching-not-delivered pipes__sqs.pipes-create-pipe-scalar-filter-pattern-rejected pipes__sqs.pipes-fifo-target-dynamic-path-group-id-resolves-from-the-source-record pipes__sqs.pipes-input-template-renders-source-fields-into-target-body
Issue-Key: pipes__sqs.pipe-filter-criteria-non-matching-not-delivered|_collect#14|Timeout|resp = sqs.receive_message(
Issue-Key: pipes__sqs.pipes-create-pipe-scalar-filter-pattern-rejected|_wait_for_messages#12|Timeout|resp = sqs.receive_message(
Issue-Key: pipes__sqs.pipes-fifo-target-dynamic-path-group-id-resolves-from-the-source-record|_receive#20|Timeout|resp = sqs.receive_message(**kwargs)
Issue-Key: pipes__sqs.pipes-input-template-renders-source-fields-into-target-body|_wait_for_messages#10|Timeout|resp = sqs.receive_message(
Issue-Commit: 6712e9b9ed86b1379f14acf29b1637f3b8f9994b

# pipes__sqs: a RUNNING pipe with an SQS source and SQS target consumes nothing and delivers nothing, and CreatePipe does not refuse a scalar filter pattern

> _Note: this issue was identified against the code base at commit `6712e9b9ed86b1379f14acf29b1637f3b8f9994b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipe

## Description
One cause, four checks (the fifth replay key listed is the delivery positive this cause is grounded in within the same group).

Cause 1 of 1: the pipes runtime is inert. CreatePipe/StartPipe report CurrentState=RUNNING, but the pipe never polls its SQS source and never writes to its SQS target, so every data-plane check in this group — positive delivery, filter-criteria suppression, FIFO dynamic MessageGroupId resolution, InputTemplate rendering, and even the CreatePipe scalar-filter-pattern refusal check that depends on a working control pipe — is killed at the 60s harness budget.
AWS: recorded behavior only for the runtime; the model fixes the surface (pipes CreatePipe takes Source, Target, RoleArn, SourceParameters.FilterCriteria.Filters[].Pattern, SourceParameters.SqsQueueParameters.BatchSize, TargetParameters.InputTemplate and TargetParameters.SqsQueueParameters.MessageGroupId; StartPipe drives CurrentState to RUNNING). In AWS a RUNNING pipe dequeues from the source, drops records that do not match FilterCriteria, renders InputTemplate into the target message body, resolves a dynamic MessageGroupId path (e.g. `$.body.<field>`) from the source record, and deletes the records it forwarded. A FilterCriteria Pattern that is a scalar rather than a JSON object is refused at CreatePipe with HTTP 400 `ValidationException`, the message naming the offending pattern from the request.
Emulator: no record is ever consumed and no message ever reaches the target queue, so none of the above is observable; the refusal check never gets past its control setup either.
Why: EventBridge Pipes reports a healthy RUNNING pipe while delivering nothing. A client that wires queue-to-queue transformation through Pipes loses every message silently, and because the negative checks also see nothing, a real filtering or validation regression is indistinguishable from correct behaviour.
Fix: the pipes runtime behind StartPipe — a source poller that applies FilterCriteria, renders InputTemplate, resolves dynamic target parameters from the source record, delivers to the target and deletes the forwarded records; plus CreatePipe's FilterCriteria pattern validation.
Traces: pipes__sqs.pipe-filter-criteria-non-matching-not-delivered, pipes__sqs.pipes-create-pipe-scalar-filter-pattern-rejected, pipes__sqs.pipes-fifo-target-dynamic-path-group-id-resolves-from-the-source-record, pipes__sqs.pipes-input-template-renders-source-fields-into-target-body

Related open issue that does not carry these replay keys: 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md.

## Evidence
All four checks: `Error: Check timed out after 60.0s`.
Expected: after StartPipe reaches CurrentState='RUNNING' and SendMessage on the source queue, ReceiveMessage on the target queue returns the rendered body (and, for the FIFO target, a MessageGroupId resolved from the source record), while a non-matching record never arrives.
Actual: nothing is consumed from the source and nothing arrives at the target.

## Steps to Reproduce
1. Create a source queue, a target queue (and a .fifo target for the group-id check) and a role pipes can assume.
2. CreatePipe(Source=<source arn>, Target=<target arn>, SourceParameters.FilterCriteria, TargetParameters.InputTemplate[, TargetParameters.SqsQueueParameters.MessageGroupId='$.body.<field>']).
3. StartPipe and wait for CurrentState='RUNNING'.
4. SendMessage on the source and poll the target — nothing arrives and the source message is never consumed.
5. CreatePipe with FilterCriteria.Filters[0].Pattern set to a scalar instead of a JSON object — no ValidationException is observed.
