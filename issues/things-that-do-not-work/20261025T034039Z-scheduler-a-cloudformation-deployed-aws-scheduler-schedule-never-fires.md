Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler.deploy-cfn-schedule-invokes-sqs-target
Issue-Shape-Id: com.amazonaws.scheduler#AWSChronosService
Issue-Key: scheduler.deploy-cfn-schedule-invokes-sqs-target|_poll_for_message#22|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=10)
Issue-Commit: 8d2004325ac14849f8dfb95f46b05c10ee176845

# A CloudFormation-deployed AWS::Scheduler::Schedule never fires, so the end-to-end CFN deploy -> fire -> SQS -> stack-delete flow never completes

> _Note: this issue was identified against the code base at commit `8d2004325ac14849f8dfb95f46b05c10ee176845`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Cause 1 of 1: a schedule created through CloudFormation (AWS::Scheduler::Schedule, with its SQS queue and scheduler-trusted IAM role in the same stack) never produces an invocation, so the deploy->fire->deliver->stack-delete round trip can never be observed. Failing check: scheduler.deploy-cfn-schedule-invokes-sqs-target.

AWS: AWS::Scheduler::Schedule is backed by the scheduler control-plane operation CreateSchedule (Smithy, scheduler/AWSChronosService: CreateSchedule POST /schedules/{Name}; required members ScheduleExpression (@length 1..256), Target, FlexibleTimeWindow; Name @length 1..64 @pattern ^[0-9a-zA-Z-_.]+$; output ScheduleArn @pattern ^arn:aws(-[a-z]+)?:scheduler:[a-z0-9\-]+:\d{12}:schedule\/[0-9a-zA-Z-_.]+\/[0-9a-zA-Z-_.]+$; errors ConflictException 409, ResourceNotFoundException 404, ValidationException 400). Recorded behavior for the composed flow: the stack reaches CREATE_COMPLETE, GetSchedule(Name) then answers 200 with State=ENABLED and an Arn ending in schedule/default/<name>, the one-time at(<fire-time>) occurrence fires once at that instant, the scheduler assumes Target.RoleArn and sends Target.Input verbatim as the SQS message body to the queue named by Fn::GetAtt [TargetQueue, Arn], and after DeleteStack both GetSchedule (ResourceNotFoundException) and GetQueueUrl (QueueDoesNotExist) refuse.

Emulator: nothing in the flow ever reaches the delivery assertion — the check exhausts the harness window and reports 'Check timed out after 60.0s'. No message is ever delivered to the stack's SQS target, so the body-equality assertion ('CFN-deployed schedule never delivered to its SQS target') and the post-DeleteStack refusals are never reached. The scheduler crate persists a ScheduleRecord and returns the ARN but has no occurrence evaluator and no target-dispatch path, and the CloudFormation resource-provider path does not drive the scheduler control plane for this resource type, so an IaC-owned schedule is doubly inert.

Why: a client that owns its schedules as IaC gets a green stack and a dead timer. Work that is supposed to be enqueued on a cron/rate/one-time schedule is silently never enqueued, with no error anywhere to signal it — control-plane reads look healthy while the data plane produces nothing, and negative expectations (disabled schedule does not fire, stack delete stops firing) appear satisfied for the wrong reason.

Fix: the scheduler crate needs the occurrence/dispatch component (services/service-com-amazonaws-scheduler-awschronosservice/src/, armed/disarmed by src/ops/create_schedule.rs, update_schedule.rs, delete_schedule.rs) covering at()/rate()/cron() evaluation, role assumption and Target.Input rendering onto the target, and the CloudFormation resource-provider mapping for AWS::Scheduler::Schedule (services/service-com-amazonaws-cloudformation-cloudformation/src/) must call that same CreateSchedule/DeleteSchedule path on stack create and delete so a CFN-created schedule is identical to an API-created one.

Related open issues that do NOT carry this check's replay key: 20261023T191008Z-scheduler-eventbridge-scheduler-never-fires-a-schedule-no-invocation.md (missing dispatch engine for API-created schedules), 20261023T191008Z-scheduler-cloudformation-stacks-containing-aws-scheduler-schedule-aws-scheduler.md (CFN does not provision scheduler resource types) and 20261025T022319Z-scheduler-cloudformation-create-complete-for-aws-scheduler-schedulegroup-but.md. They name this check only in prose, so this flow is not gated by any of them; it must be re-replayed in its own right. The previously fixed 20261024T050214Z-scheduler-cloudformation-stack-reports-create-complete-but-aws-scheduler.md addressed the ScheduleGroup read-back only, not schedule firing, and this check does not fail on a refusal, so it is not a reopen of that fix.

Traces: scheduler.deploy-cfn-schedule-invokes-sqs-target.

## Evidence
Check scheduler.deploy-cfn-schedule-invokes-sqs-target: [mismatch] 'Error: Check timed out after 60.0s' — no stage of the flow produced the delivery the check waits for.
Expected: cloudformation.create_stack with AWS::SQS::Queue + AWS::IAM::Role (trusting scheduler.amazonaws.com, inline sqs:SendMessage on the queue ARN) + AWS::Scheduler::Schedule{ScheduleExpression: at(<now+4min>), FlexibleTimeWindow:{Mode:OFF}, Target:{Arn: GetAtt Queue.Arn, RoleArn: GetAtt Role.Arn, Input: <body>}} reaches CREATE_COMPLETE; scheduler.get_schedule(Name) -> 200 State=ENABLED, Arn ending 'schedule/default/<name>'; one SQS message arrives whose Body equals the template's MessageBody parameter; after delete_stack, get_schedule -> ResourceNotFoundException and sqs.get_queue_url -> QueueDoesNotExist.
Actual: the target queue stays empty for the whole window and the check is cut off at the 60 s cap; the Body assertion, the post-delete ResourceNotFoundException and the post-delete QueueDoesNotExist are never evaluated.

## Steps to Reproduce
1. Build a CloudFormation template with Resources: TargetQueue (AWS::SQS::Queue, QueueName from a parameter), SchedulerRole (AWS::IAM::Role, RoleName from a parameter, AssumeRolePolicyDocument allowing sts:AssumeRole for Service scheduler.amazonaws.com, inline policy allowing sqs:SendMessage on {Fn::GetAtt: [TargetQueue, Arn]}), and MySchedule (AWS::Scheduler::Schedule, Name from a parameter, ScheduleExpression {Fn::Sub: 'at(${FireTime})'} with FireTime about 4 minutes in the future, FlexibleTimeWindow {Mode: OFF}, Target {Arn: GetAtt TargetQueue.Arn, RoleArn: GetAtt SchedulerRole.Arn, Input: a marker string}).
2. cloudformation.create_stack(StackName=s, TemplateBody=<template>, Parameters=[...], Capabilities=['CAPABILITY_NAMED_IAM'], OnFailure='DELETE') and wait for the stack_create_complete waiter.
3. scheduler.get_schedule(Name=<schedule name>) and assert State == 'ENABLED' and Arn ends with 'schedule/default/<schedule name>'.
4. sqs.get_queue_url(QueueName=<queue name>) and poll sqs.receive_message(QueueUrl=..., MaxNumberOfMessages=1, WaitTimeSeconds=10) until 3 minutes past the fire time: on AWS exactly one message arrives whose Body is the marker; on the emulator the queue stays empty and the check is killed at the 60 s cap.
5. cloudformation.delete_stack(StackName=s), wait stack_delete_complete, then scheduler.get_schedule(Name=...) must raise ResourceNotFoundException and sqs.get_queue_url(QueueName=...) must raise QueueDoesNotExist.
