Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__events__sqs.xsvc-elbv2-1-create-target-group-cloudtrail-event-reaches-queue-with-request-shape elbv2__events__sqs.xsvc-elbv2-1-delete-target-group-rule-receives-nothing-from-create elbv2__events__sqs.xsvc-elbv2-1-duplicate-target-group-name-rejected-and-event-carries-error-code elbv2__events__sqs.xsvc-elbv2-1-input-transformer-delivers-only-the-extracted-target-group-name elbv2__events__sqs.xsvc-elbv2-1-unpermitted-queue-target-routes-event-to-dead-letter-queue
Issue-Key: elbv2__events__sqs.xsvc-elbv2-1-create-target-group-cloudtrail-event-reaches-queue-with-request-shape|_prove_channel_live#7|Timeout|time.sleep(_POLL_S)
Issue-Key: elbv2__events__sqs.xsvc-elbv2-1-delete-target-group-rule-receives-nothing-from-create|_prove_channel_live#10|Timeout|time.sleep(_POLL_S)
Issue-Key: elbv2__events__sqs.xsvc-elbv2-1-duplicate-target-group-name-rejected-and-event-carries-error-code|_prove_channel_live#7|Timeout|time.sleep(_POLL_S)
Issue-Key: elbv2__events__sqs.xsvc-elbv2-1-input-transformer-delivers-only-the-extracted-target-group-name|_prove_channel_live#9|Timeout|time.sleep(_POLL_S)
Issue-Key: elbv2__events__sqs.xsvc-elbv2-1-unpermitted-queue-target-routes-event-to-dead-letter-queue|_prove_channel_live#10|Timeout|time.sleep(_POLL_S)
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# No ELBv2 control-plane event ever reaches an EventBridge rule's SQS target: all five ELBv2/EventBridge/SQS checks time out at 60s with nothing delivered

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetGroup

## Description
One root cause, grounded in 5 failing behavioral checks whose id's leading token is the composite 'elbv2__events__sqs'.

Cause 1 of 1: an ELBv2 control-plane call produces no event that an EventBridge rule can match, so an SQS target (and its dead-letter queue) never receives anything and every check hangs until its 60s budget expires.
AWS: ELBv2 is an awsQuery service whose CloudTrail event source is elasticloadbalancing.amazonaws.com (Smithy awsService.cloudTrailEventSource on com.amazonaws.elasticloadbalancingv2, version 2015-12-01). A CreateTargetGroup call — whether served 200 with {TargetGroups} or refused 400 with a modeled awsQuery code such as DuplicateTargetGroupName — is recorded as an AWS API Call via CloudTrail event; an EventBridge rule matching that source delivers a message to its SQS target within seconds, a non-matching pattern delivers nothing, an input transformer delivers only the extracted field, and a target whose queue policy does not permit events.amazonaws.com routes the attempt to the rule's dead-letter queue. Recorded behavior only for the delivery latency the checks budget.
Emulator: all five checks reach the 60s ceiling with no delivery and no assertion ever satisfied ('Check timed out after 60.0s'). Hypothesis: nothing is emitted in the first place, because the ELBv2 awsQuery action table resolves no operation at version 2015-12-01 — a direct CreateTargetGroup call in this same run is refused with Error.Code 'InvalidAction' and message 'Could not find operation CreateTargetGroup for version 2015-12-01' (filed separately for the 'elbv2' token) — so the checks poll a queue that can never fill. Even a refused call must produce an event carrying the exception-suffixed error code, which these checks also assert.
Why: a client cannot build any event-driven automation on ELBv2 changes: no rule fires, no input transformation is observable, a mismatched pattern is indistinguishable from a broken bus, and an unpermitted target produces no dead-letter evidence. A 60s hang rather than an error also makes the gap look like a latency problem instead of a missing feature.
Fix: ELBv2 control-plane event emission (CloudTrail/EventBridge event generation on ELBv2 success and modeled refusal) plus the EventBridge rule -> SQS target delivery path including input transformation and dead-letter routing; the ELBv2 awsQuery dispatch table must be registered first or there is no call to emit from.
Traces: the 5 'elbv2__events__sqs.*' check ids in trace_ids.

## Evidence
All five checks report, as their only detail, 'Error: Check timed out after 60.0s' with verdict mismatch:
- elbv2__events__sqs.xsvc-elbv2-1-create-target-group-cloudtrail-event-reaches-queue-with-request-shape
- elbv2__events__sqs.xsvc-elbv2-1-delete-target-group-rule-receives-nothing-from-create
- elbv2__events__sqs.xsvc-elbv2-1-duplicate-target-group-name-rejected-and-event-carries-error-code
- elbv2__events__sqs.xsvc-elbv2-1-input-transformer-delivers-only-the-extracted-target-group-name
- elbv2__events__sqs.xsvc-elbv2-1-unpermitted-queue-target-routes-event-to-dead-letter-queue
Supporting observation from the same suite run: a direct ELBv2 CreateTargetGroup is answered (InvalidAction) 'Could not find operation CreateTargetGroup for version 2015-12-01', so no ELBv2 control-plane call can succeed or be audited.
Expected: an SQS ReceiveMessage on the rule's target queue returns the CloudTrail-shaped event for the CreateTargetGroup call (and, for the negative cases, nothing on the non-matching rule and a message on the dead-letter queue) well inside the 60s budget.

## Steps to Reproduce
1. Boot the emulator.
2. Create an SQS queue, an EventBridge rule matching source 'aws.elasticloadbalancing' / detail-type 'AWS API Call via CloudTrail' with the queue as target, and grant events.amazonaws.com SendMessage on the queue.
3. Call boto3.client('elbv2').create_target_group(Name='sim2real-tg', Protocol='HTTP', Port=80, VpcId=<vpc>); then poll sqs.receive_message on the target queue.
4. No message ever arrives (and the ELBv2 call itself is refused with Error.Code 'InvalidAction'); the behavioral checks hit their 60s ceiling.
5. Re-replay the 5 check ids in trace_ids.
