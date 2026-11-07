Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3__sqs.event-source-mapping-writes-one-far-end-object-per-request-id
Issue-Key: ec2__lambda__s3__sqs.event-source-mapping-writes-one-far-end-object-per-request-id|_poll#19|Timeout|time.sleep(interval_s)
Issue-Commit: e4e3e5befb4453536c7145f8d5553fe1cd0770b8

# ec2__lambda__s3__sqs: an SQS event source mapping reports Enabled but never hands a batch to the function, so the per-request-id far-end write never happens (check cut at the 60 s cap)

> _Note: this issue was identified against the code base at commit `e4e3e5befb4453536c7145f8d5553fe1cd0770b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause, grounded in the only failing check in this suite run that is neither marked ALREADY FILED nor OWNED by an open issue.

Cause 1 of 1: the Lambda event-source-mapping poller for an SQS queue never delivers. The check creates a queue (VisibilityTimeout 20), a function whose handler throws on each message's first delivery and then writes exactly one S3 object per message request id, and an event source mapping with BatchSize=1; it polls the mapping to a terminal state, sends three messages, and then waits for (a) one 'THREW' log record per request id and (b) exactly one analytics/<request-id>.json object per request id.

AWS: Smithy (lambda, awsgirapiservice): CreateEventSourceMapping — input EventSourceArn (Arn), FunctionName (required), BatchSize @range 1..10000, Enabled; output EventSourceMappingConfiguration carrying UUID, State, StateTransitionReason, LastProcessingResult; HTTP POST /2015-03-31/event-source-mappings with success code 202. The model states the mapping's existence and State but not its delivery semantics, so for the delivery itself this is recorded behavior only: the recording shows the mapping reaching State 'Enabled', each sent message being handed to the function, a handler exception leaving the message on the queue so SQS redelivers it after the visibility timeout, and the retry succeeding — ending with exactly one far-end S3 object per request id, all inside the suite's 60 s budget. No refusal and no non-2xx is involved on AWS: every API call in the scenario succeeds.

Emulator: the check is cut off with 'Error: Check timed out after 60.0s'. The control-plane calls succeed and the mapping reports a terminal State, but no batch is ever handed to the function: no handler log record for any of the three request ids ever appears and no object is ever written to the analytics bucket, so neither the first-delivery-failure assertion nor the one-object-per-request-id assertion is ever reached.

Why: a client that wires an SQS queue to a Lambda through an event source mapping sees a healthy, Enabled mapping while its queue is silently never drained. Work is accepted and lost: no invocation, no retry, no idempotent far-end output, and no error anywhere to diagnose it. Any reconciliation built on 'one output object per request id' never converges.

Fix: the Lambda event-source-mapping SQS poller (receive -> batch to BatchSize -> invoke -> delete on success / leave for redelivery on handler error) and the mapping state/LastProcessingResult it reports, together with the SQS visibility-timeout redelivery path it depends on.

Related already-open issue (does NOT carry this failure's replay key in this run, so it is named here rather than relied on): 20261104T044826Z-ec2-lambda-s3-sqs-createvpcendpoint-refuses-route-table-associations-and-masks-the.md describes the same non-delivery as its Cause 3 of 3 behind two CreateVpcEndpoint causes; this run's failure signature for the check did not match it, so the cause is re-filed standalone and should be closed together with it.

Traces: ec2__lambda__s3__sqs.event-source-mapping-writes-one-far-end-object-per-request-id.

## Evidence
Expected (AWS): CreateEventSourceMapping(EventSourceArn=<queue arn>, FunctionName=<fn>, BatchSize=1) -> 202, mapping reaches State 'Enabled'; each SendMessage is delivered to the function; the handler's first-delivery exception leaves the message on the queue and SQS redelivers it after VisibilityTimeout; the handler's successful retry writes one object per request id, so the bucket holds exactly ['analytics/<req1>.json', 'analytics/<req2>.json', 'analytics/<req3>.json'] (keys derived from the request ids in the request, not from any single recorded run) within the 60 s harness budget.
Actual (emulator): 'Error: Check timed out after 60.0s'. The mapping is created and reports a terminal state, but no handler log record ('THREW <request id>') appears for any message and the analytics prefix stays empty, so the check never reaches a verdict.

## Steps to Reproduce
1. sqs:CreateQueue with Attributes={'VisibilityTimeout':'20'}; read QueueArn via GetQueueAttributes.
2. Create an IAM role allowing s3:PutObject on the target bucket and sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the queue, and a Lambda function (Timeout=10, MemorySize=256) whose handler raises on a message's first delivery and otherwise writes s3://<bucket>/analytics/<request-id>.json.
3. lambda:CreateEventSourceMapping(EventSourceArn=<queue arn>, FunctionName=<fn>, BatchSize=1); poll GetEventSourceMapping until State is Enabled.
4. SendMessage three times with distinct request ids.
5. Poll the function's logs for one 'THREW <request-id>' record per id, then ListObjectsV2 on the analytics/ prefix. Observe that no invocation ever happens and no object is ever written, so the scenario never settles.
