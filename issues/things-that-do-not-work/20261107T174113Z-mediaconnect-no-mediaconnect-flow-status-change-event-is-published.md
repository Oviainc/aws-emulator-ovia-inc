Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediaconnect.flow-state-change-eventbridge-pattern-match-delivers mediaconnect.flow-state-change-eventbridge-pattern-mismatch-no-delivery mediaconnect.eventbridge-missing-queue-policy-blocks-delivery
Issue-Shape-Id: com.amazonaws.mediaconnect#MediaConnect
Issue-Key: mediaconnect.flow-state-change-eventbridge-pattern-match-delivers|run#17|AssertionError|assert messages, "no MediaConnect flow status change event delivered to SQS"
Issue-Key: mediaconnect.flow-state-change-eventbridge-pattern-mismatch-no-delivery|run#26|AssertionError|assert matched_messages, "matching-pattern rule never delivered the event (setup is broken)"
Issue-Key: mediaconnect.eventbridge-missing-queue-policy-blocks-delivery|run#22|AssertionError|assert allowed_messages, "the properly-authorized queue never received the event"
Issue-Commit: b865e0575531bfd25013a0fbecff1c37458c2050

# No 'MediaConnect Flow Status Change' event is published to EventBridge on a flow status transition, so every rule target receives nothing

> _Note: this issue was identified against the code base at commit `b865e0575531bfd25013a0fbecff1c37458c2050`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartFlow

## Description
One net-new root cause surfaced by this replay of the behavioral suite against the MediaConnect emulator (15/28 checks pass; the other 12 failing checks are all marked OWNED by an already-open issue and are not repeated here).

NOTE ON AN EXISTING ISSUE: open issue `20261022T132818Z-mediaconnect-router-interface-union-payload-rejected-no-flow.md` describes this same behavior as its "Cause 2 of 6" and lists these three check ids in its Issue-Trace-Ids. However the replay report did NOT mark these three checks as OWNED by it: that issue's recorded replay keys for them are `...|_poll_for_messages#N|Timeout|resp = sqs.receive_message(...)`, and the checks no longer fail that way. The failure signature has MOVED — the SQS/EventBridge polls now complete inside their budget and return an empty list, so the failure is now an `AssertionError` on the positive-control assertion in `run`, not a harness timeout. The underlying MediaConnect defect is unchanged and is restated here with current evidence and current replay keys so it is gated by checks that actually reproduce it; a fixer should treat this and that issue's Cause 2 as the same work.

Cause 1 of 1: a flow status transition publishes no event onto the EventBridge default bus.

AWS: recorded behavior only — the Smithy model (`services/service-com-amazonaws-mediaconnect-mediaconnect/smithy-model/com-amazonaws-mediaconnect-mediaconnect.json`) describes StartFlow (POST /v1/flows/start/{FlowArn}, success 202, output {flowArn, status}) but says nothing about eventing. The recordings show that whenever a flow's Status transitions, MediaConnect publishes an event to the account's default EventBridge bus with `source` = `aws.mediaconnect`, `detail-type` = `MediaConnect Flow Status Change`, the flow's ARN in `resources`, and the status in `detail` (`detail.currentStatus` is STARTING or ACTIVE after StartFlow). An ENABLED rule whose pattern matches that source/detail-type/resource, with an SQS target whose queue policy grants `events.amazonaws.com` `sqs:SendMessage`, receives that event; a target queue with no such policy receives nothing; a rule naming a detail-type MediaConnect never emits receives nothing.

Emulator: StartFlow's handler (`services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/start_flow.rs`) only mutates stored flow status — it calls `flow.begin_transition(STATUS_STARTING, STATUS_ACTIVE, now)` and returns 202 — and publishes nothing. No event is ever placed on the emulator's EventBridge default bus, so no rule matches and no SQS target is written. The three checks below each poll their correctly-authorized, correctly-matching positive-control queue and receive zero messages before asserting.

Why: a client that drives MediaConnect through events never observes a flow coming up. EventBridge rules, Lambda/SQS fan-out, status-change alarms and any operator tooling keyed on `MediaConnect Flow Status Change` stay silent forever while the control plane reports the flow ACTIVE, so a reconciliation loop or a workflow that waits on the event hangs rather than failing. It also makes the two negative-delivery contracts untestable against this emulator: because the positive control never delivers, a client cannot distinguish "EventBridge correctly withheld this event" (no queue policy / non-matching pattern) from "MediaConnect emits no events at all" — the emulator looks compliant on the negative half for the wrong reason.

Fix: the flow status-transition path, not any single serializer. The transition sites — `src/ops/start_flow.rs` and `src/ops/stop_flow.rs`, plus the deferred settle in `src/state.rs` (`settle_due_transitions`, which is where STARTING→ACTIVE actually lands) — must publish the flow-status-change event onto the emulator's EventBridge default bus for every status change, carrying source `aws.mediaconnect`, detail-type `MediaConnect Flow Status Change`, the flow ARN in `resources` and the new status in `detail.currentStatus`. Emitting only on the accepted call and not on the clock-driven settle would still drop the ACTIVE transition.

Traces: mediaconnect.flow-state-change-eventbridge-pattern-match-delivers, mediaconnect.flow-state-change-eventbridge-pattern-mismatch-no-delivery, mediaconnect.eventbridge-missing-queue-policy-blocks-delivery.

## Evidence
Replay report: mediaconnect 15/28 passed (54%), 13 failed (7 mismatch, 6 error). 10 of the 13 are marked OWNED by open issues and skipped; the 3 below are this issue.

All three checks perform the same setup: create_flow(Source={'Name':'source1','Protocol':'rtp','WhitelistCidr':'203.0.113.0/24'}) -> put_rule(EventPattern={'source':['aws.mediaconnect'],'detail-type':['MediaConnect Flow Status Change'],'resources':[flowArn]}, State='ENABLED') -> set_queue_attributes granting events.amazonaws.com sqs:SendMessage on the target queue (Condition ArnEquals aws:SourceArn = the rule ARN) -> put_targets(the queue ARN) -> start_flow(FlowArn=flowArn) -> poll receive_message.

Expected (AWS): within the poll window the positive-control queue holds at least one message whose body is an EventBridge envelope with source == 'aws.mediaconnect', detail-type == 'MediaConnect Flow Status Change', the flow ARN present in resources, and detail.currentStatus in ('STARTING','ACTIVE').

Actual (emulator): the queue is empty for the whole window; the poll returns [].

[mismatch] mediaconnect.flow-state-change-eventbridge-pattern-match-delivers
  check_mediaconnect_flow_state_change_eventbridge_pattern_match_delivers.py:61
  assert messages, "no MediaConnect flow status change event delivered to SQS"
  AssertionError: no MediaConnect flow status change event delivered to SQS
  (120s positive-control poll on the single matching, authorized queue)

[mismatch] mediaconnect.flow-state-change-eventbridge-pattern-mismatch-no-delivery
  check_mediaconnect_flow_state_change_eventbridge_pattern_mismatch_no_delivery.py:91
  assert matched_messages, "matching-pattern rule never delivered the event (setup is broken)"
  AssertionError: matching-pattern rule never delivered the event (setup is broken)
  (the rule matching 'MediaConnect Flow Status Change' delivers nothing, so the companion assertion that a rule matching 'MediaConnect Flow Alert' delivers nothing is never reached)

[mismatch] mediaconnect.eventbridge-missing-queue-policy-blocks-delivery
  check_mediaconnect_eventbridge_missing_queue_policy_blocks_delivery.py:90
  assert allowed_messages, "the properly-authorized queue never received the event"
  AssertionError: the properly-authorized queue never received the event
  (one rule, two SQS targets; the queue WITH the events.amazonaws.com sqs:SendMessage policy receives nothing, so the assertion that the unpolicied queue stays at ApproximateNumberOfMessages 0 is never reached)

Source evidence — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/start_flow.rs lines 146-218: the handler resolves state, calls store.settle_due_transitions(now), looks up the flow, and on an accepted start calls flow.begin_transition(STATUS_STARTING, STATUS_ACTIVE, now) before returning StartFlowOutput. There is no event publication anywhere in the handler.

Signature change vs. the already-open issue: that issue recorded these three as harness timeouts inside _poll_for_messages ('Check timed out after 60.0s'). They now fail fast on the positive-control assertion with empty poll results, which isolates the defect to MediaConnect's missing emission rather than to slow SQS/EventBridge responses.

## Steps to Reproduce
Boot the emulator and run run_behavioral_suite, or reproduce directly with botocore clients pointed at the emulator endpoint:

1. mc = client('mediaconnect'); events = client('events'); sqs = client('sqs').
2. flow_arn = mc.create_flow(Name='f1', Source={'Name':'source1','Protocol':'rtp','WhitelistCidr':'203.0.113.0/24'})['Flow']['FlowArn']
3. queue_url = sqs.create_queue(QueueName='q1')['QueueUrl']; queue_arn = sqs.get_queue_attributes(QueueUrl=queue_url, AttributeNames=['QueueArn'])['Attributes']['QueueArn']
4. rule_arn = events.put_rule(Name='r1', State='ENABLED', EventPattern=json.dumps({'source':['aws.mediaconnect'],'detail-type':['MediaConnect Flow Status Change'],'resources':[flow_arn]}))['RuleArn']
5. sqs.set_queue_attributes(QueueUrl=queue_url, Attributes={'Policy': json.dumps({'Version':'2012-10-17','Statement':[{'Effect':'Allow','Principal':{'Service':'events.amazonaws.com'},'Action':'sqs:SendMessage','Resource':queue_arn,'Condition':{'ArnEquals':{'aws:SourceArn':rule_arn}}}]})})
6. events.put_targets(Rule='r1', Targets=[{'Id':'queue-target','Arn':queue_arn}])
7. mc.start_flow(FlowArn=flow_arn)  -> 202, Status STARTING
8. Poll sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5) for up to 120s.

Observed: the queue stays empty indefinitely, while mc.describe_flow(FlowArn=flow_arn)['Flow']['Status'] advances STARTING -> ACTIVE. Expected: at least one message whose JSON body has source 'aws.mediaconnect', detail-type 'MediaConnect Flow Status Change', flow_arn in resources, and detail.currentStatus in ('STARTING','ACTIVE').

To see that the clock-driven settle must emit too: repeat and poll only AFTER describe_flow reports ACTIVE — the ACTIVE transition produces no event either.
