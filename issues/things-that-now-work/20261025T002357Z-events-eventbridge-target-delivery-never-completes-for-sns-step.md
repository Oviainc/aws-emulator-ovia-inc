Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.cross-region-bus-target-delivers-to-the-other-regions-rules-with-the-source-region-and-a-new-event-id events.express-and-standard-state-machine-targets-start-with-the-envelope-as-input-named-after-the-event-id events.input-transformer-array-paths-render-elements-slices-and-null-exactly events.put-events-trace-header-reaches-sqs-targets-as-aws-trace-header events.put-targets-same-id-new-arn-moves-delivery-to-the-new-queue events__sqs.put-targets-same-id-upsert-moves-delivery-to-the-new-queue events.sns-target-publishes-the-compact-envelope-and-a-plain-text-template-keeps-its-quotes events.sns-target-on-a-kms-encrypted-topic-dead-letters-as-no-permissions-until-the-key-policy-grants-events events.twelve-rules-matching-one-event-each-deliver-their-own-copy-and-a-near-miss-rule-none events__sqs.events-bus-policy-explicit-deny-rejects-entire-putevents-batch events.log-group-target-dead-letters-events-timed-too-old-or-too-far-ahead-while-a-current-event-is-written
Issue-Key: events.cross-region-bus-target-delivers-to-the-other-regions-rules-with-the-source-region-and-a-new-event-id|run#23|Timeout|for msg in sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10,
Issue-Key: events.express-and-standard-state-machine-targets-start-with-the-envelope-as-input-named-after-the-event-id|_receive#20|Timeout|for msg in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: events.input-transformer-array-paths-render-elements-slices-and-null-exactly|_drain#15|Timeout|resp = sqs.receive_message(
Issue-Key: events.put-events-trace-header-reaches-sqs-targets-as-aws-trace-header|_drain#21|Timeout|resp = sqs.receive_message(
Issue-Key: events.put-targets-same-id-new-arn-moves-delivery-to-the-new-queue|_drain#20|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: events__sqs.put-targets-same-id-upsert-moves-delivery-to-the-new-queue|_collect#20|Timeout|resp = sqs.receive_message(
Issue-Key: events.sns-target-publishes-the-compact-envelope-and-a-plain-text-template-keeps-its-quotes|_receive#15|Timeout|for msg in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: events.sns-target-on-a-kms-encrypted-topic-dead-letters-as-no-permissions-until-the-key-policy-grants-events|_receive#17|Timeout|for msg in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5,
Issue-Key: events.twelve-rules-matching-one-event-each-deliver-their-own-copy-and-a-near-miss-rule-none|_receive#10|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__sqs.events-bus-policy-explicit-deny-rejects-entire-putevents-batch|_poll_until_denied#15|Timeout|time.sleep(3.0)
Issue-Key: eventbridgev2|PutResourcePolicy|InvalidParameterException
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# EventBridge target delivery never completes for SNS, Step Functions, cross-region buses, input transformers, trace headers, multi-rule fan-out and re-pointed targets, and a log-group target cannot be granted at all (9 causes)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Nine delivery paths produce no terminal effect. Each check establishes its own positive control and then reads at the far end of a target; in all nine the read never succeeds within the check's budget and the replay cuts the check off at 60 s (cause 9 fails earlier, on a refused grant). These are distinct target types and distinct transformation paths, so they are distinct causes.

Cause 1 of 9 — cross-region event-bus target. AWS: a rule whose target is an event bus in another region, with a RoleArn, re-publishes the event onto that bus, where it is matched by that region's rules; the delivered copy carries the SOURCE region in `region` and a NEW event id. Emulator: the other region's rules never receive anything. Fix: the event-bus target delivery path in services/service-com-amazonaws-eventbridge-awsevents/src/ops/put_events.rs / the target dispatcher. Traces: events.cross-region-bus-target-delivers-to-the-other-regions-rules-with-the-source-region-and-a-new-event-id.

Cause 2 of 9 — Step Functions targets (both STANDARD and EXPRESS). AWS: an `arn:aws:states:...:stateMachine:...` target starts an execution whose input is the full event envelope and whose execution name is derived from the event id. Emulator: no execution is observable. Fix: the states target in the dispatcher. Traces: events.express-and-standard-state-machine-targets-start-with-the-envelope-as-input-named-after-the-event-id.

Cause 3 of 9 — InputTransformer array paths. AWS: InputPathsMap entries selecting array elements, slices and absent fields render into the target's input exactly (an absent field renders as the documented empty value). Emulator: the transformed payload never arrives at the target. Fix: the InputTransformer renderer in the dispatcher. Traces: events.input-transformer-array-paths-render-elements-slices-and-null-exactly.

Cause 4 of 9 — X-Ray trace header propagation to SQS targets. AWS: a PutEvents entry carrying TraceHeader delivers to an SQS target with the `AWSTraceHeader` system message attribute set. Emulator: nothing is delivered, so the attribute can never be observed. Fix: the SQS target delivery path and its system-attribute mapping. Traces: events.put-events-trace-header-reaches-sqs-targets-as-aws-trace-header.

Cause 5 of 9 — re-pointing an existing target Id to a new ARN. AWS: PutTargets with the SAME Id and a new Arn replaces the target; subsequent events go to the new queue and not the old one. Emulator: neither queue receives, so the move is not observable. Fix: target upsert in .../src/ops/put_targets.rs and the dispatcher's target lookup. Traces: events.put-targets-same-id-new-arn-moves-delivery-to-the-new-queue, events__sqs.put-targets-same-id-upsert-moves-delivery-to-the-new-queue.

Cause 6 of 9 — SNS targets. AWS: an SNS topic target is published to with the compact event envelope, and an InputTransformer plain-text template keeps its quoting; a KMS-encrypted topic whose key policy does not grant events.amazonaws.com dead-letters the delivery with ERROR_CODE NO_PERMISSIONS until the key policy is fixed. Emulator: no publish and no dead-letter record is observable. Fix: the SNS target in the dispatcher and its DLQ error classification. Traces: events.sns-target-publishes-the-compact-envelope-and-a-plain-text-template-keeps-its-quotes, events.sns-target-on-a-kms-encrypted-topic-dead-letters-as-no-permissions-until-the-key-policy-grants-events.

Cause 7 of 9 — fan-out to many rules matching one event. AWS: twelve rules that each match one event each receive their own copy, and a near-miss rule receives none. Emulator: the copies never arrive. Fix: rule evaluation/fan-out in the dispatcher. Traces: events.twelve-rules-matching-one-event-each-deliver-their-own-copy-and-a-near-miss-rule-none.

Cause 8 of 9 — a bus-policy explicit Deny must reject the WHOLE PutEvents batch. AWS: an explicit Deny in the bus policy rejects the entire batch, so no entry in it is delivered. Emulator: the check cannot complete its reads; combined with the authorization gap filed separately, the batch-level Deny semantics are unobservable. Fix: the batch-level authorization decision in put_events.rs. Traces: events__sqs.events-bus-policy-explicit-deny-rejects-entire-putevents-batch.

Cause 9 of 9 — a CloudWatch Logs group cannot be granted to EventBridge because PutResourcePolicy demands policyName. AWS: CloudWatch Logs `PutResourcePolicy` is called with policyDocument and a resourceArn to scope the grant to one log group; policyName is not required in that form. Emulator: answers `InvalidParameterException` / "policyName is required", so the log-group target can never be granted and the check's event-age dead-lettering behaviour is unreachable. Fix: the CloudWatch Logs PutResourcePolicy handler, services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_resource_policy.rs (required-parameter check). Related but not the same defect as the already-open 20261023T113357Z-logs-cloudwatch-logs-accepts-requests-real-aws-refuses-putresourcepolicy.md, which is about Logs ACCEPTING a PutResourcePolicy AWS refuses; this is the inverse refusal. Traces: events.log-group-target-dead-letters-events-timed-too-old-or-too-far-ahead-while-a-current-event-is-written.

## Evidence
Causes 1-8: the replay reports "Check timed out after 60.0s" for each listed check. In every one the check first issues PutRule/PutTargets successfully (FailedEntryCount 0) and PutEvents successfully (FailedEntryCount 0, an EventId returned), then polls the target's far end — the other region's queue, a Step Functions execution list, the transformed SQS body, the AWSTraceHeader attribute, the new queue after a same-Id upsert, the SNS-subscribed endpoint or the dead-letter queue, the twelve per-rule queues — and the effect never appears. Expected: the target effect inside the check's budget (these checks allow 45-180 s of delivery latency; the suite's 60 s cap is reached with nothing delivered).

Cause 9: "botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the PutResourcePolicy operation: policyName is required". Expected: 200, the resource policy stored for the named resourceArn.

## Steps to Reproduce
1. CreateEventBus in region B; in region A PutRule + PutTargets with Arn=<region B bus arn> and a RoleArn; PutEvents in A -> region B's rule target never receives.
2. PutTargets with Arn=<state machine arn> + RoleArn; PutEvents -> ListExecutions stays empty.
3. PutTargets with InputTransformer using array-element/slice/absent paths; PutEvents -> the SQS target never receives.
4. PutEvents(Entries=[{..., 'TraceHeader': 'Root=1-...'}]) to a rule with an SQS target -> no message, so no AWSTraceHeader.
5. PutTargets(Rule=r, Targets=[{Id:'t1', Arn:<queueA>}]) then PutTargets(Rule=r, Targets=[{Id:'t1', Arn:<queueB>}]); PutEvents -> neither queue receives.
6. PutTargets with Arn=<SNS topic arn> (plain, and KMS-encrypted without an events grant); PutEvents -> no publish and no DLQ record.
7. Create 12 rules each matching a distinct event plus one near-miss rule, each with its own queue; PutEvents 12 matching events -> no queue receives.
8. Put a bus policy with an explicit Deny for the caller; PutEvents a batch -> batch-level outcome unobservable.
9. logs.PutResourcePolicy(policyDocument=<doc>, resourceArn=<log group arn>) -> 400 InvalidParameterException "policyName is required".
