Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__scheduler.lambda-invokes-scheduler-create-schedule-positive lambda__scheduler.lambda-invokes-scheduler-delete-schedule-positive lambda__scheduler.lambda-invokes-scheduler-get-schedule-positive lambda__scheduler.lambda-scheduler-create-schedule-missing-permission-negative lambda__scheduler.lambda-scheduler-create-schedule-wrong-group-negative lambda__scheduler.lambda-scheduler-list-schedules-missing-permission-negative lambda__scheduler.lambda-scheduler-update-schedule-group-positive
Issue-Key: lambda__scheduler.lambda-invokes-scheduler-create-schedule-positive|run#8|AssertionError|assert out["ok"] is True, f"create_schedule via lambda failed: {out}"
Issue-Key: lambda__scheduler.lambda-invokes-scheduler-delete-schedule-positive|run#10|AssertionError|assert out["ok"] is True, f"delete_schedule via lambda failed: {out}"
Issue-Key: lambda__scheduler.lambda-invokes-scheduler-get-schedule-positive|run#10|AssertionError|assert out["ok"] is True, f"get_schedule via lambda failed: {out}"
Issue-Key: lambda__scheduler.lambda-scheduler-create-schedule-missing-permission-negative|run#9|AssertionError|assert out["error_code"] == "AccessDeniedException", f"expected AccessDeniedException, got: {out['error_code']}"
Issue-Key: lambda__scheduler.lambda-scheduler-create-schedule-wrong-group-negative|run#10|AssertionError|assert out["error_code"] == "ResourceNotFoundException", (
Issue-Key: lambda__scheduler.lambda-scheduler-list-schedules-missing-permission-negative|run#10|AssertionError|assert out["error_code"] == "AccessDeniedException", f"expected AccessDeniedException, got: {out}"
Issue-Key: lambda__scheduler.lambda-scheduler-update-schedule-group-positive|run#8|AssertionError|assert out_group["ok"] is True, f"create_schedule_group via lambda failed: {out_group}"
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# lambda__scheduler: boto3.client('scheduler') cannot be built inside a Lambda, so every EventBridge Scheduler check fails with UnknownServiceError

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
One root cause behind 7 failing checks — all of this service's failures.

Cause 1 of 1: the in-process Lambda Python runtime's bundled AWS SDK ships a four-service data catalog, so `boto3.client('scheduler')` raises before any request is sent; no EventBridge Scheduler call made from a handler ever reaches the service.
AWS: a handler that builds a `scheduler` client can call CreateSchedule, GetSchedule, DeleteSchedule, ListSchedules and CreateScheduleGroup. Smithy (services/service-com-amazonaws-scheduler-awschronosservice): CreateSchedule is `POST /schedules/{Name}` returning 200 with a required `ScheduleArn`, and declares ValidationException (400), ConflictException (409), ResourceNotFoundException (404), ServiceQuotaExceededException (402), ThrottlingException (429), InternalServerException (500); GetSchedule is `GET /schedules/{Name}` with ResourceNotFoundException (404), ValidationException (400), ThrottlingException (429), InternalServerException (500). So: naming a `GroupName` that does not exist is refused with HTTP 404 and error code `ResourceNotFoundException`, and an execution role without `scheduler:CreateSchedule`/`scheduler:ListSchedules` is refused with HTTP 403 and error code `AccessDeniedException` whose message is the fixed `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: scheduler:CreateSchedule on resource: <schedule-arn> because no identity-based policy allows the scheduler:CreateSchedule action` (variable parts from the request and the role). `UnknownServiceError` is not an AWS error at all — it is a client-side botocore error.
Emulator: every handler raises errorType `UnknownServiceError`, errorMessage `Unknown service: 'scheduler'. Valid service names are: sqs, events, eventbridge, dynamodb`, with FunctionError `Unhandled` on a 200 invoke response. Both the positive checks (which expect a ScheduleArn) and the negative checks (which expect AccessDeniedException or ResourceNotFoundException) fail, and the negatives fail for a reason that says nothing about Scheduler or IAM.
Why: no function can create, read, update or delete a schedule — EventBridge Scheduler is unusable from Lambda, which is its most common driver. Worse, every negative path collapses into one client-construction error, so application code that distinguishes a missing schedule group (ResourceNotFoundException) from a missing permission (AccessDeniedException) cannot work, and a reviewer cannot tell either apart.
Fix: the in-process Lambda Python runtime's bundled AWS SDK — its service-model data catalog must carry every service the emulator serves (scheduler among them) and the client factory must stop refusing them — plus the Scheduler handlers' IAM authorization so a missing grant yields AccessDeniedException rather than nothing. A related open issue, 20261106T131020Z-bedrock-agent-runtime-lambda-s3vectors-the-lambda-python.md, describes the same four-service catalog for a different service; it does not carry any of these replay keys.
Traces: the 7 failing checks in trace_ids, under `### lambda__scheduler`.

## Evidence
Expected (AWS): CreateSchedule from the handler -> HTTP 200 with ScheduleArn; a nonexistent GroupName -> HTTP 404 `ResourceNotFoundException`; a role without the scheduler action -> HTTP 403 `AccessDeniedException` with `... is not authorized to perform: scheduler:CreateSchedule on resource: <arn> because no identity-based policy allows the scheduler:CreateSchedule action`.

Actual (emulator):
- lambda-invokes-scheduler-create-schedule-positive -> create_schedule via lambda failed: {'ok': False, 'error_code': 'UnknownServiceError', 'error_message': "Unknown service: 'scheduler'. Valid service names are: sqs, events, eventbridge, dynamodb"}
- lambda-invokes-scheduler-get-schedule-positive -> same UnknownServiceError
- lambda-invokes-scheduler-delete-schedule-positive -> same UnknownServiceError
- lambda-scheduler-update-schedule-group-positive -> create_schedule_group via lambda failed: same UnknownServiceError
- lambda-scheduler-create-schedule-missing-permission-negative -> expected AccessDeniedException, got: UnknownServiceError
- lambda-scheduler-list-schedules-missing-permission-negative -> expected AccessDeniedException, got: {'ok': False, 'error_code': 'UnknownServiceError', ...}
- lambda-scheduler-create-schedule-wrong-group-negative -> expected ResourceNotFoundException for a nonexistent GroupName, got: UnknownServiceError

## Steps to Reproduce
1. Create a Lambda execution role granting scheduler:* plus iam:PassRole.
2. Create a python Lambda whose handler does `boto3.client('scheduler').create_schedule(Name=..., ScheduleExpression='rate(1 hour)', FlexibleTimeWindow={'Mode':'OFF'}, Target={...})`.
3. Invoke it. Expected: 200 with ScheduleArn. Observed: FunctionError Unhandled, UnknownServiceError `Unknown service: 'scheduler'. Valid service names are: sqs, events, eventbridge, dynamodb`.
4. Repeat with GroupName set to a nonexistent group (expect ResourceNotFoundException) and with a role granting no scheduler action (expect AccessDeniedException) — both give the same UnknownServiceError.
