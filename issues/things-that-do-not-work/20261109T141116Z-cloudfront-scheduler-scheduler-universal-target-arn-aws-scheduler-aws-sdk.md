Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__scheduler.xsvc-cloudfront-3-scheduler-one-time-invalidation-applies-exact-paths cloudfront__scheduler.xsvc-cloudfront-3-scheduler-role-denied-invalidation-routes-to-dlq
Issue-Key: cloudfront__scheduler.xsvc-cloudfront-3-scheduler-one-time-invalidation-applies-exact-paths|_poll_until#22|AssertionError|raise AssertionError(f"timed out waiting for {description} after {timeout_s}s (last: {last!r})")
Issue-Key: cloudfront__scheduler.xsvc-cloudfront-3-scheduler-role-denied-invalidation-routes-to-dlq|_poll_until#28|AssertionError|raise AssertionError(f"timed out waiting for {description} after {timeout_s}s (last: {last!r})")
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# scheduler universal target arn:aws:scheduler:::aws-sdk:cloudfront:createInvalidation never issues the SDK call, so no invalidation is ever created

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
This issue carries 1 root cause, in the EventBridge Scheduler occurrence/dispatch component's handling of UNIVERSAL targets.

Cause 1 of 1: a schedule whose Target.Arn is a universal target (arn:aws:scheduler:::aws-sdk:<service>:<operation>) is accepted and reads back correctly, but the SDK call it names is never issued — here cloudfront:createInvalidation.
AWS: Smithy (com.amazonaws.scheduler) — Target.Arn -> TargetArn, required, @length 1..1600, documented as "The Amazon Resource Name (ARN) of the target"; the Target shape's own documentation states EventBridge Scheduler "supports templated target that invoke common API operations, as well as universal targets that you can customize to invoke over 6,000 API operations across more than 270 services". Target.Input is "The text, or well-formed JSON, passed to the target". So when Target.Arn is a universal target ARN, at each occurrence Scheduler assumes Target.RoleArn and calls that service operation with Target.Input as the request body. For arn:aws:scheduler:::aws-sdk:cloudfront:createInvalidation with an Input carrying DistributionId and InvalidationBatch, CloudFront must record an invalidation on that distribution whose Paths are exactly the submitted ones (recorded behavior: ListInvalidations on the distribution reports the new invalidation within the delivery window).
Emulator: CreateSchedule returns 200, GetSchedule echoes the universal Target.Arn and State=ENABLED, and then nothing happens: ListInvalidations on the target distribution stays empty for the whole 420 s poll window in both checks (the second check fails on its PERMITTED positive control, before it can even reach its denied/DLQ subject). No CloudFront invalidation is ever created by a schedule.
Why: universal targets are how clients drive the ~6,000 operations Scheduler has no templated parameter block for. A client that schedules a cache invalidation sees a healthy control plane (200 on CreateSchedule, ENABLED on GetSchedule) and no invalidation ever runs — the scheduled work silently never happens, with no error anywhere to observe. It also makes the denied-role/DLQ behaviour of universal targets untestable, since that check cannot pass its positive control.
Fix: the occurrence/dispatch component of the scheduler crate (services/service-com-amazonaws-scheduler-awschronosservice/src/, armed by src/ops/create_schedule.rs and update_schedule.rs). It must recognise a Target.Arn of the form arn:aws:scheduler:::aws-sdk:<service>:<operation>, resolve <service>/<operation> to the emulated service's operation, and issue that call with Target.Input as the request payload under Target.RoleArn. Related open issue 20261023T191008Z-scheduler-eventbridge-scheduler-never-fires-a-schedule-no-invocation.md concerns the dispatcher generally and does not carry these checks; a dispatcher that handles only templated SQS/Lambda targets leaves this cause open.
Traces: cloudfront__scheduler.xsvc-cloudfront-3-scheduler-one-time-invalidation-applies-exact-paths, cloudfront__scheduler.xsvc-cloudfront-3-scheduler-role-denied-invalidation-routes-to-dlq.

## Evidence
Replay of simparity-behavioral-aws-v1: cloudfront__scheduler 0/2 (both failed).

[mismatch] cloudfront__scheduler.xsvc-cloudfront-3-scheduler-one-time-invalidation-applies-exact-paths
AssertionError: timed out waiting for the schedule <name> to create an invalidation on <distribution id> after 420.0s (last: [])

[mismatch] cloudfront__scheduler.xsvc-cloudfront-3-scheduler-role-denied-invalidation-routes-to-dlq
AssertionError: timed out waiting for the permitted schedule to invalidate ['/<prefix>/good/*'] on <distribution id> after 420.0s (last: [])

Expected: within the delivery window, cloudfront.list_invalidations(DistributionId=<id>) reports an invalidation created by the schedule, whose InvalidationBatch.Paths.Items are exactly the paths in Target.Input.
Actual: the invalidation list stays empty ('last: []') for 420 s in both runs, while GetSchedule reports State=ENABLED with the universal Target.Arn intact. The distribution ids, schedule names and path prefixes above are one run's values, not requirements.

## Steps to Reproduce
1. cloudfront.create_distribution(...) and note its Id; iam.create_role trusting scheduler.amazonaws.com with an inline policy allowing cloudfront:CreateInvalidation on that distribution.
2. scheduler.create_schedule(Name=n, ScheduleExpression='at(<now+~1min>)' (or 'rate(1 minute)'), ScheduleExpressionTimezone='UTC', FlexibleTimeWindow={'Mode':'OFF'}, State='ENABLED', Target={'Arn':'arn:aws:scheduler:::aws-sdk:cloudfront:createInvalidation', 'RoleArn':role_arn, 'Input': json.dumps({'DistributionId': <id>, 'InvalidationBatch': {'Paths': {'Quantity': 1, 'Items': ['/<prefix>/good/*']}, 'CallerReference': <unique>}})}).
3. scheduler.get_schedule(Name=n) -> Target.Arn round-trips and State == 'ENABLED' (this already passes).
4. Poll cloudfront.list_invalidations(DistributionId=<id>) past the nominal instant. On AWS an invalidation appears whose Paths equal the submitted Items. On the emulator the list stays empty indefinitely (observed empty after 420 s).
Checks: cloudfront__scheduler.xsvc-cloudfront-3-scheduler-one-time-invalidation-applies-exact-paths, cloudfront__scheduler.xsvc-cloudfront-3-scheduler-role-denied-invalidation-routes-to-dlq.
