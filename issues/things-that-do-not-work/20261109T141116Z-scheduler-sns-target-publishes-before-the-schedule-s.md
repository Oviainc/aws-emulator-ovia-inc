Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler__sns.scheduler-sns-target-delivers-the-notification-envelope-and-a-denied-role-withholds-it
Issue-Key: scheduler__sns.scheduler-sns-target-delivers-the-notification-envelope-and-a-denied-role-withholds-it|run#24|AssertionError|assert early <= 5, (
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# scheduler SNS target publishes BEFORE the schedule's own reported scheduled time (Mode=OFF fires early)

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
This issue carries 1 root cause, in the EventBridge Scheduler occurrence computation as observed through an SNS target.

Cause 1 of 1: a FlexibleTimeWindow Mode=OFF schedule publishes to its SNS target before the nominal instant the service itself reports as the occurrence's scheduled time.
AWS: Smithy (com.amazonaws.scheduler) — FlexibleTimeWindow is required on CreateSchedule; Mode -> FlexibleTimeWindowMode (required) "Determines whether the schedule is invoked within a flexible time window". With Mode=OFF there is no window at all: the occurrence is invoked AT the nominal instant derived from ScheduleExpression (+ ScheduleExpressionTimezone), never before it. The nominal instant is also the value Scheduler renders into the payload for the <aws.scheduler.scheduled-time> context attribute, so the timestamp the notification is stamped with by SNS must be at or after the scheduled time the same payload reports.
Emulator: SNS stamps the notification 58.0 s BEFORE the instant Scheduler reports as the occurrence's scheduled time — the emulator fires the occurrence early while rendering the correct nominal instant into the payload, so the delivery and its own metadata contradict each other.
Why: a Mode=OFF schedule is the strongest timing contract this service offers; a client uses it precisely when the work must not run before a given instant. Subscribers that act on <aws.scheduler.scheduled-time> (idempotency keys, windowing, ordering) see a notification that claims a future scheduled time and has already been delivered, so deduplication and window logic built on it is wrong. The schedule reads back exactly as configured, so nothing in the API reports the divergence.
Fix: the occurrence computation in the dispatch component (services/service-com-amazonaws-scheduler-awschronosservice/src/) must not dispatch before the nominal instant — the same instant it already renders as <aws.scheduler.scheduled-time> must be the floor for the actual publish, for Mode=OFF and as the lower clamp of the Mode=FLEXIBLE window.
Traces: scheduler__sns.scheduler-sns-target-delivers-the-notification-envelope-and-a-denied-role-withholds-it.

## Evidence
Replay of simparity-behavioral-aws-v1: scheduler__sns 0/1.

[mismatch] scheduler__sns.scheduler-sns-target-delivers-the-notification-envelope-and-a-denied-role-withholds-it
AssertionError: SNS stamped the notification at 2026-11-09T12:53:10.007665+00:00, 58.0s BEFORE the instant Scheduler reported as its scheduled time (2026-11-09T12:54:08+00:00); a Mode=OFF schedule cannot publish early

Expected: the SNS publish timestamp is at or after the scheduled time the payload itself reports. Actual: ~58 s before it. The two timestamps above are one run's values; the requirement is the ordering between them, not the numbers. The check fails on this positive control, so its second subject (a denied role withholding the notification) was not reached.

## Steps to Reproduce
1. sns.create_topic and subscribe an SQS queue to it (so the publish timestamp is observable); iam.create_role trusting scheduler.amazonaws.com with an inline sns:Publish policy on the topic ARN.
2. scheduler.create_schedule(Name=n, ScheduleExpression='at(<now+~1min>)' (or a pinned cron minute), ScheduleExpressionTimezone='UTC', FlexibleTimeWindow={'Mode':'OFF'}, State='ENABLED', Target={'Arn': topic_arn, 'RoleArn': role_arn, 'Input': json.dumps({'scheduled': '<aws.scheduler.scheduled-time>'})}).
3. Receive the notification from the subscribed queue and compare its stamped timestamp with the <aws.scheduler.scheduled-time> value rendered into the payload. On AWS the stamp is >= the scheduled time; on the emulator it precedes it by ~58 s.
Check: scheduler__sns.scheduler-sns-target-delivers-the-notification-envelope-and-a-denied-role-withholds-it.
