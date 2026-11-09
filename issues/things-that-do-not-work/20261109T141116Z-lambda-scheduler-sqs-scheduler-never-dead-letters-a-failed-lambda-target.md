Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-role-without-invoke-permission-denied-to-dlq lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty lambda__scheduler__sqs.xsvc-lambda-3-scheduler-deleted-lambda-target-dead-letters-once
Issue-Key: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-role-without-invoke-permission-denied-to-dlq|_await_message#22|AssertionError|raise AssertionError(f"{description}: nothing arrived within {timeout_s:.0f}s")
Issue-Key: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty|_await_message#24|AssertionError|raise AssertionError(f"{description}: nothing arrived within {timeout_s:.0f}s")
Issue-Key: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-deleted-lambda-target-dead-letters-once|_await_message#28|AssertionError|raise AssertionError(f"{description}: nothing arrived within {timeout_s:.0f}s")
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# scheduler never dead-letters a failed Lambda-target invocation: 2 causes — a denied Target.RoleArn and a deleted target function both leave DeadLetterConfig.Arn empty

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
This issue carries 2 root causes, both in the EventBridge Scheduler occurrence/dispatch component's failed-delivery path. Each must be closed before this issue is submitted; each cause lists the failing check ids it is grounded in. Neither is marked ALREADY FILED or OWNED in the replay report.

=== Cause 1 of 2: an occurrence refused because Target.RoleArn lacks lambda:InvokeFunction is never delivered to Target.DeadLetterConfig.Arn ===
Failing checks: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-role-without-invoke-permission-denied-to-dlq, lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty
AWS: Smithy (com.amazonaws.scheduler) — Target.RoleArn (required) is "the IAM role that EventBridge Scheduler will use for this target when the schedule is invoked"; Target.DeadLetterConfig -> DeadLetterConfig, Arn @pattern ^arn:aws(-[a-z]+)?:sqs:[a-z0-9\-]+:\d{12}:[a-zA-Z0-9\-_]+$, documented as the queue to which "EventBridge Scheduler delivers failed events that could not be successfully delivered to a target". An invocation the assumed role is not permitted to make is such a failed event: the function is not invoked, the RetryPolicy is applied, and the event is then placed on the dead-letter queue. Recorded behavior: at least one message appears on the DLQ for the denied schedule, and the permitted twin's own DLQ stays empty.
Emulator: the dead-letter queue stays empty — the check polls 150 s and receives nothing, in two independent checks (one whose subject IS the denied-to-DLQ path, one whose denied twin is its negative control alongside a permitted schedule).
Why: the DLQ is the only observable signal that a scheduled invocation failed; Scheduler reports nothing to the caller at occurrence time. A client that alarms on its schedule DLQ gets permanent silence, so a schedule that never does its work is indistinguishable from a healthy one. (The emulator additionally invokes despite the missing permission — filed separately under lambda__scheduler — but even once that is fixed, the failed occurrence must land on the DLQ, which is this cause.)
Fix: the occurrence/dispatch component of the scheduler crate (services/service-com-amazonaws-scheduler-awschronosservice/src/) must, after a delivery failure and the exhaustion of RetryPolicy, SendMessage the failed-event payload to Target.DeadLetterConfig.Arn.
Traces: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-role-without-invoke-permission-denied-to-dlq, lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty.

=== Cause 2 of 2: an occurrence whose target Lambda function no longer exists is never dead-lettered either ===
Failing check: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-deleted-lambda-target-dead-letters-once
AWS: when Target.Arn names a Lambda function that has been deleted, the Invoke call is refused by Lambda (ResourceNotFoundException) — a delivery failure, so the occurrence is retried per RetryPolicy and then delivered to Target.DeadLetterConfig.Arn. Recorded behavior: exactly one dead-letter message for that occurrence, and the schedule itself stays ENABLED (Scheduler does not delete a schedule whose target has gone).
Emulator: nothing reaches the dead-letter queue within 150 s, so a schedule pointing at a target that no longer exists fails completely silently.
Why: a deleted or renamed target is the most common way a working schedule breaks in production. On AWS the DLQ makes it visible (and once-only, so it can be alarmed without flooding); on the emulator the schedule reports ENABLED forever and the client has no way to learn that every occurrence is being dropped.
Fix: the same failed-delivery path — a target-call refusal (including ResourceNotFoundException from the target service) must be classified as a delivery failure and routed to DeadLetterConfig.Arn exactly once per occurrence after RetryPolicy is exhausted, without altering the schedule's State.
Traces: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-deleted-lambda-target-dead-letters-once.

## Evidence
Replay of simparity-behavioral-aws-v1: lambda__scheduler__sqs 0/4 (3 failed, 1 unloadable). Names below are one run's values, not requirements.

Cause 1:
  [mismatch] lambda__scheduler__sqs.xsvc-lambda-3-scheduler-role-without-invoke-permission-denied-to-dlq — AssertionError: the denied invocation reaching its DLQ: nothing arrived within 150s
  [mismatch] lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty — AssertionError: twin schedule <name> dead-lettering its denied invoke: nothing arrived within 150s
  Expected: >= 1 message on Target.DeadLetterConfig.Arn for the schedule whose role lacks lambda:InvokeFunction.
  Actual: 0 messages over a 150 s poll window in both runs.

Cause 2:
  [mismatch] lambda__scheduler__sqs.xsvc-lambda-3-scheduler-deleted-lambda-target-dead-letters-once — AssertionError: the deleted-target invocation reaching its DLQ: nothing arrived within 150s
  Expected: exactly one dead-letter message for the occurrence whose target function has been deleted.
  Actual: 0 messages over a 150 s poll window.

## Steps to Reproduce
Common setup: lambda.create_function for the target; sqs.create_queue for the dead-letter queue and read its QueueArn; iam.create_role trusting scheduler.amazonaws.com with sqs:SendMessage on the DLQ.

Cause 1: give the role NO lambda:InvokeFunction permission on the function. scheduler.create_schedule(Name=n, ScheduleExpression='rate(1 minute)', FlexibleTimeWindow={'Mode':'OFF'}, State='ENABLED', Target={'Arn': function_arn, 'RoleArn': role_arn, 'Input': json.dumps({...}), 'DeadLetterConfig': {'Arn': dlq_arn}, 'RetryPolicy': {'MaximumRetryAttempts': 0}}). Poll sqs.receive_message(QueueUrl=dlq_url) past the first occurrence: on AWS a dead-letter message arrives; on the emulator none does (observed: nothing within 150 s). Checks lambda__scheduler__sqs.xsvc-lambda-3-scheduler-role-without-invoke-permission-denied-to-dlq and (as the denied twin beside a permitted schedule) lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty.

Cause 2: give the role lambda:InvokeFunction on the function and create the same schedule WITH DeadLetterConfig; then lambda.delete_function(FunctionName=<name>) while the schedule stays ENABLED. Poll the DLQ past the next occurrence: on AWS exactly one dead-letter message arrives for that occurrence and get_schedule still reports State='ENABLED'; on the emulator the DLQ stays empty (observed: nothing within 150 s). Check lambda__scheduler__sqs.xsvc-lambda-3-scheduler-deleted-lambda-target-dead-letters-once.
