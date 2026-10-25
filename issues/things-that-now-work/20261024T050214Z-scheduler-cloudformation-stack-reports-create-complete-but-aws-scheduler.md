Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler.deploy-cloudformation-schedule-group-membership-behaves
Issue-Shape-Id: com.amazonaws.scheduler#AWSChronosService
Issue-Key: scheduler|GetScheduleGroup|ResourceNotFoundException
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# CloudFormation stack reports CREATE_COMPLETE but AWS::Scheduler::ScheduleGroup is never created (GetScheduleGroup 404)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation stack that declares an AWS::Scheduler::ScheduleGroup (with a nested AWS::Scheduler::Schedule, an AWS::SQS::Queue and an AWS::IAM::Role) now reaches CREATE_COMPLETE, but the schedule group it declares does not exist in EventBridge Scheduler afterwards.

AWS: once such a stack reaches CREATE_COMPLETE, the declared resources exist in Scheduler and are readable through its own API: GetScheduleGroup(Name=<group>) returns HTTP 200 with State ACTIVE, and ListSchedules(GroupName=<group>) returns exactly the nested schedule with GroupName set to that group and State ENABLED. Smithy (scheduler model, com.amazonaws.scheduler): CreateScheduleGroup POST /schedule-groups/{Name} -> 200 {ScheduleGroupArn}; GetScheduleGroup GET /schedule-groups/{Name} -> 200 {Arn, Name, State: ScheduleGroupState, CreationDate, LastModificationDate} with ResourceNotFoundException (HTTP 404) only when the group does not exist; ListSchedules GET /schedules with GroupName as the ScheduleGroup query parameter. Group names are ScheduleGroupName, len 1..64, pattern ^[0-9a-zA-Z-_.]+$. That CFN creates the resource before reporting CREATE_COMPLETE is recorded behavior.
Emulator: the create_stack waiter succeeds and DescribeStacks reports CREATE_COMPLETE, and the very next read fails: GetScheduleGroup answers HTTP 404 ResourceNotFoundException with message 'Schedule group <group> does not exist.' (example from the replay: 'Schedule group sim2real-behav-cfn-group-47dbce67f842 does not exist.'). No group, and therefore no nested schedule, was provisioned, so the group-membership assertions (State ACTIVE, ListSchedules showing the schedule under that group) are never reachable.
Why: a client that owns its schedule groups and schedules through IaC gets a green stack and no resources. Every subsequent control-plane read 404s, group membership and firing never happen, and stack UPDATE/DELETE reconciliation operates on resources that were never created — the stack status actively lies about what exists.
Fix: the CloudFormation resource-provider mapping for AWS::Scheduler::ScheduleGroup (and the nested AWS::Scheduler::Schedule) — the resource dispatch table in services/service-com-amazonaws-cloudformation-cloudformation/src/ must invoke this service's CreateScheduleGroup/CreateSchedule handlers (services/service-com-amazonaws-scheduler-awschronosservice/src/ops/create_schedule_group.rs, create_schedule.rs) during stack create, record them in stack state, and call the matching deletes on stack delete; CREATE_COMPLETE must be reported only after those calls succeed.
Related open issue: 20261023T191008Z-scheduler-cloudformation-stacks-containing-aws-scheduler-schedule-aws-scheduler.md describes the same provisioning gap, but for this check it recorded a different symptom (the stack driven to CREATE_FAILED, so the stack_create_complete waiter raised a terminal failure). The replay no longer shows that: the stack now completes successfully and the divergence has moved to the post-create read, so this check is not covered by that issue's recorded key and is filed here.
Traces: scheduler.deploy-cloudformation-schedule-group-membership-behaves.

## Evidence
Check scheduler.deploy-cloudformation-schedule-group-membership-behaves [error].
Expected: after create_stack + stack_create_complete + DescribeStacks == CREATE_COMPLETE, scheduler.get_schedule_group(Name=<group>) -> HTTP 200 with State == 'ACTIVE', and scheduler.list_schedules(GroupName=<group>) -> exactly one schedule with that Name, GroupName == <group>, State == 'ENABLED'.
Actual: the stack reports CREATE_COMPLETE, then botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetScheduleGroup operation: Schedule group sim2real-behav-cfn-group-47dbce67f842 does not exist. (HTTP 404). The ListSchedules membership assertions are never reached. Group/stack names and the account id in the example differ per run; the requirement is the 404 vs 200 ACTIVE divergence.

## Steps to Reproduce
1. Build a template with Resources: {ScheduleGroup: AWS::Scheduler::ScheduleGroup {Name: {Ref: GroupName}}, Queue: AWS::SQS::Queue {QueueName: {Ref: QueueName}}, SchedulerRole: AWS::IAM::Role trusting scheduler.amazonaws.com for sts:AssumeRole, Schedule: AWS::Scheduler::Schedule {Name: {Ref: ScheduleName}, GroupName: {Ref: ScheduleGroup}, ScheduleExpression: 'rate(1 day)', FlexibleTimeWindow: {Mode: 'OFF'}, State: 'ENABLED', Target: {Arn: {Fn::GetAtt: [Queue, Arn]}, RoleArn: {Fn::GetAtt: [SchedulerRole, Arn]}}}} and Parameters GroupName/ScheduleName/QueueName.
2. cloudformation.create_stack(StackName=<s>, TemplateBody=<template>, Parameters=[...], Capabilities=['CAPABILITY_IAM'], OnFailure='DELETE').
3. Wait on the stack_create_complete waiter; describe_stacks reports CREATE_COMPLETE.
4. scheduler.get_schedule_group(Name=<GroupName>) -> HTTP 404 ResourceNotFoundException 'Schedule group <GroupName> does not exist.' instead of HTTP 200 with State 'ACTIVE'.
5. scheduler.list_schedules(GroupName=<GroupName>) likewise does not contain <ScheduleName> with GroupName=<GroupName> and State 'ENABLED'.
