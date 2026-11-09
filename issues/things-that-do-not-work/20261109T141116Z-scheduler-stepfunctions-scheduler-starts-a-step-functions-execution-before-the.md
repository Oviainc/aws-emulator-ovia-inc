Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler__stepfunctions.scheduler-schedule-starts-a-state-machine-execution-observed-at-step-functions
Issue-Key: scheduler__stepfunctions.scheduler-schedule-starts-a-state-machine-execution-observed-at-step-functions|run#30|AssertionError|assert early <= _EARLY_SLACK_S, (
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# scheduler starts a Step Functions execution BEFORE the schedule's nominal instant (Mode=OFF fires early)

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
This issue carries 1 root cause, in the EventBridge Scheduler occurrence computation as observed through a Step Functions target.

Cause 1 of 1: a FlexibleTimeWindow Mode=OFF schedule starts its state-machine execution before the nominal instant derived from its ScheduleExpression.
AWS: Smithy (com.amazonaws.scheduler) — FlexibleTimeWindow is required on CreateSchedule and Mode -> FlexibleTimeWindowMode (required) "Determines whether the schedule is invoked within a flexible time window". With Mode=OFF there is no window: the occurrence is invoked AT the nominal instant computed from ScheduleExpression (+ ScheduleExpressionTimezone), and never before it. The execution Step Functions records therefore has a startDate at or after that instant.
Emulator: Step Functions records the execution 13.6 s BEFORE the schedule's nominal instant — the occurrence is dispatched early even though no flexible window was configured.
Why: a Mode=OFF schedule is the strongest timing contract the service offers, and a client uses it exactly when the workflow must not begin before a given instant (input data not yet written, an upstream window not yet closed). The state machine starts against state that does not exist yet, and nothing in the API reports it: GetSchedule echoes the expression and Mode faithfully. Any assertion that a Mode=OFF schedule cannot fire early passes against AWS and fails here.
Fix: the occurrence computation in the dispatch component (services/service-com-amazonaws-scheduler-awschronosservice/src/) must treat the nominal instant as a hard floor for dispatch — exactly at nominal for Mode=OFF, and as the lower clamp of [nominal, nominal + MaximumWindowInMinutes] for Mode=FLEXIBLE.
Traces: scheduler__stepfunctions.scheduler-schedule-starts-a-state-machine-execution-observed-at-step-functions.

## Evidence
Replay of simparity-behavioral-aws-v1: scheduler__stepfunctions 0/1.

[mismatch] scheduler__stepfunctions.scheduler-schedule-starts-a-state-machine-execution-observed-at-step-functions
AssertionError: Step Functions recorded the execution starting at 2026-11-09T12:53:15.423000+00:00, 13.6s BEFORE the schedule's nominal instant 2026-11-09T12:53:29+00:00; a Mode=OFF schedule cannot fire early

Expected: the execution's startDate is at or after the nominal instant of the occurrence. Actual: ~13.6 s before it. The two timestamps above are one run's values; the requirement is the ordering between them.

## Steps to Reproduce
1. stepfunctions.create_state_machine and note its ARN; iam.create_role trusting scheduler.amazonaws.com with an inline states:StartExecution policy on that ARN.
2. scheduler.create_schedule(Name=n, ScheduleExpression='at(<a pinned instant ~1 min in the future>)', ScheduleExpressionTimezone='UTC', FlexibleTimeWindow={'Mode':'OFF'}, State='ENABLED', Target={'Arn': state_machine_arn, 'RoleArn': role_arn, 'Input': json.dumps({...})}).
3. Poll stepfunctions.list_executions / describe_execution and read the execution's startDate; compare it with the nominal instant from the expression. On AWS startDate >= nominal; on the emulator it precedes nominal (observed 13.6 s early).
Check: scheduler__stepfunctions.scheduler-schedule-starts-a-state-machine-execution-observed-at-step-functions.
