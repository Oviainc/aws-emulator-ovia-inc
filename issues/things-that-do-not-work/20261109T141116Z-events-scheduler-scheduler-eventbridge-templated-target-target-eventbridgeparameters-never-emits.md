Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__scheduler.eventbridge-templated-target-detail-type-and-source-reach-the-bus
Issue-Key: events__scheduler.eventbridge-templated-target-detail-type-and-source-reach-the-bus|run#26|AssertionError|assert envelopes, (
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# scheduler EventBridge templated target (Target.EventBridgeParameters) never emits an event to the bus, so no rule ever matches

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
This issue carries 1 root cause, in the EventBridge Scheduler occurrence/dispatch component.

Cause 1 of 1: a schedule whose Target.Arn is an EventBridge event bus and which carries Target.EventBridgeParameters never performs the PutEvents call, so no event reaches the bus and no rule on it ever matches.
AWS: Smithy (com.amazonaws.scheduler) — Target.EventBridgeParameters is documented as "The templated target type for the EventBridge PutEvents API operation"; Target.Arn (required, @length 1..1600) is the bus ARN and Target.Input is "The text, or well-formed JSON, passed to the target" (for a templated EventBridge target the input must be well-formed JSON). At each occurrence Scheduler assumes Target.RoleArn and calls events:PutEvents on that bus with the entry's DetailType and Source taken from Target.EventBridgeParameters and Detail taken from Target.Input, so a rule on the bus whose event pattern matches that source/detail-type fires and delivers to its own target (recorded behavior: the probe value carried in Target.Input arrives at the rule's SQS target within the delivery window).
Emulator: CreateSchedule returns 200 and GetSchedule reports State=ENABLED with the submitted Target, but no event carrying the probe ever reaches the rule's queue within 420 s. The check explicitly separates the two possibilities and reports that the schedule existed and the EMISSION is what is missing: the PutEvents call is never issued.
Why: this is the fan-out pattern for scheduled work — a schedule publishes one event and any number of rules react. A client gets a control plane that reports a healthy ENABLED schedule and a bus that never sees an event, so every downstream consumer silently never runs and there is no error to observe anywhere.
Fix: the occurrence/dispatch component of the scheduler crate (services/service-com-amazonaws-scheduler-awschronosservice/src/). When Target.Arn names an event bus it must issue events:PutEvents under Target.RoleArn with Source and DetailType from Target.EventBridgeParameters and Detail from Target.Input. Related open issue 20261023T191008Z-scheduler-eventbridge-scheduler-never-fires-a-schedule-no-invocation.md concerns the dispatcher generally and does not carry this check; a dispatcher covering only SQS/Lambda targets leaves this cause open.
Traces: events__scheduler.eventbridge-templated-target-detail-type-and-source-reach-the-bus.

## Evidence
Replay of simparity-behavioral-aws-v1: events__scheduler 0/1.

[mismatch] events__scheduler.eventbridge-templated-target-detail-type-and-source-reach-the-bus
AssertionError: the schedule's nominal instant was <T> and no event carrying probe '<probe>' reached the rule's queue within 420.0s; GetSchedule reports state 'ENABLED', so the schedule existed and the EMISSION is what is missing

Expected: one PutEvents entry on the bus per occurrence, with Source and DetailType from Target.EventBridgeParameters and Detail from Target.Input; the bus rule matching that source/detail-type delivers it to its SQS target, where the probe value from Target.Input is observable.
Actual: the rule's queue receives nothing for 420 s after the nominal instant; GetSchedule still reports State=ENABLED. The probe string and nominal timestamp above are one run's values, not requirements.

## Steps to Reproduce
1. events.create_event_bus(Name=b); sqs.create_queue for the rule target; events.put_rule(EventBusName=b, EventPattern={'source': [<src>], 'detail-type': [<dt>]}) and events.put_targets pointing at the queue.
2. iam.create_role trusting scheduler.amazonaws.com with an inline events:PutEvents policy on the bus ARN.
3. scheduler.create_schedule(Name=n, ScheduleExpression='at(<now+~1min>)' or 'rate(1 minute)', ScheduleExpressionTimezone='UTC', FlexibleTimeWindow={'Mode':'OFF'}, State='ENABLED', Target={'Arn': bus_arn, 'RoleArn': role_arn, 'Input': json.dumps({'probe': <probe>}), 'EventBridgeParameters': {'DetailType': <dt>, 'Source': <src>}}).
4. scheduler.get_schedule(Name=n) -> State == 'ENABLED' with the submitted Target (this already passes).
5. Poll sqs.receive_message on the rule's queue past the nominal instant. On AWS a message arrives whose detail carries the probe, with the configured source and detail-type. On the emulator nothing arrives (observed: nothing within 420 s).
Check: events__scheduler.eventbridge-templated-target-detail-type-and-source-reach-the-bus.
