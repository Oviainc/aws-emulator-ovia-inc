Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler.deploy-cfn-schedule-group-with-nested-schedule
Issue-Shape-Id: com.amazonaws.scheduler#AWSChronosService
Issue-Key: scheduler.deploy-cfn-schedule-group-with-nested-schedule|run#15|AssertionError|assert any(g["Name"] == group_name and g["State"] == "ACTIVE" for g in groups), (
Issue-Commit: 41791a89e1771892a99a46c0bbc6fb2f1f57a7b6

# CloudFormation CREATE_COMPLETE for AWS::Scheduler::ScheduleGroup, but the group never appears in ListScheduleGroups

> _Note: this issue was identified against the code base at commit `41791a89e1771892a99a46c0bbc6fb2f1f57a7b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListScheduleGroups

## Description
Cause 1 of 1: a CloudFormation stack that declares an AWS::Scheduler::ScheduleGroup together with a nested AWS::Scheduler::Schedule (plus an AWS::SQS::Queue target and an AWS::IAM::Role) reaches CREATE_COMPLETE, but EventBridge Scheduler's own listing API does not enumerate the declared group afterwards.

AWS: once such a stack reaches CREATE_COMPLETE, the declared Scheduler resources exist and are enumerable through the Scheduler API. Smithy (com.amazonaws.scheduler, AWSChronosService): ListScheduleGroups GET /schedule-groups -> 200 with required member ScheduleGroups (ScheduleGroupList); its NamePrefix query member is ScheduleGroupNamePrefix, @length 1..64, pattern ^[0-9a-zA-Z-_.]+$, and the operation is paginated (inputToken/outputToken NextToken, pageSize MaxResults 1..100, items ScheduleGroups). Each entry carries Name and State (ScheduleGroupState), which is ACTIVE for a successfully created group. ListSchedules GET /schedules takes GroupName as the ScheduleGroup query parameter and returns the schedules nested in that group with GroupName set to it. That CloudFormation provisions the resource before reporting CREATE_COMPLETE is recorded behavior.
Emulator: the create_stack waiter succeeds (stack reaches CREATE_COMPLETE), and the immediately following read through the list_schedule_groups paginator with NamePrefix=<group> returns no entry whose Name is the declared group with State ACTIVE — the check fails on `assert any(g["Name"] == group_name and g["State"] == "ACTIVE" ...)` with 'schedule group <group> not visible via ListScheduleGroups'. The group was never provisioned, so the follow-on assertions (ListSchedules showing the nested schedule with GroupName == <group>, and both resources being gone after stack delete) are unreachable.
Why: a client that manages its schedule groups and schedules through IaC gets a green stack and no resources. Discovery by listing returns nothing, membership and firing never happen, and stack update/delete reconciliation runs against resources that were never created — the stack status reports success for work that did not occur.
Fix: the CloudFormation resource-provider mapping for AWS::Scheduler::ScheduleGroup and the nested AWS::Scheduler::Schedule — the resource dispatch in the cloudformation service crate must invoke the scheduler crate's CreateScheduleGroup/CreateSchedule handlers during stack create, register them in Scheduler's state so the list/get paths see them, and invoke the matching deletes on stack delete; CREATE_COMPLETE must be reported only after those calls succeed.
Related open issue: 20261024T050214Z-scheduler-cloudformation-stack-reports-create-complete-but-aws-scheduler.md describes the same provisioning gap observed through GetScheduleGroup's 404. It does not carry this check's replay key — this check diverges on the ListScheduleGroups enumeration path (a 200 response whose ScheduleGroups list omits the group), which is a different observation point and a different assertion, so it is filed here rather than assumed covered.
Traces: scheduler.deploy-cfn-schedule-group-with-nested-schedule.

## Evidence
Check scheduler.deploy-cfn-schedule-group-with-nested-schedule [mismatch].
Expected: after create_stack + the stack_create_complete waiter returns, scheduler.get_paginator('list_schedule_groups').paginate(NamePrefix=<group>) yields an entry with Name == <group> and State == 'ACTIVE' (HTTP 200), and scheduler.list_schedules(GroupName=<group>) returns exactly one schedule with Name == <schedule> and GroupName == <group>.
Actual: the stack reaches CREATE_COMPLETE, then the paginated ListScheduleGroups result contains no such entry; the check raises AssertionError: 'schedule group sim2real-sched-grp-efda4b801615 not visible via ListScheduleGroups' (file check_scheduler_deploy_cfn_schedule_group_with_nested_schedule.py line 114). The subsequent ListSchedules and post-delete ResourceNotFoundException assertions are never reached. Stack/group/schedule/queue/role names in the example are generated per run; the requirement is that a group declared by a CREATE_COMPLETE stack is enumerated by ListScheduleGroups with State ACTIVE.

## Steps to Reproduce
1. Build a template with Parameters GroupName/ScheduleName/QueueName/RoleName and Resources: MyGroup = AWS::Scheduler::ScheduleGroup {Name: {Ref: GroupName}}; TargetQueue = AWS::SQS::Queue {QueueName: {Ref: QueueName}}; SchedulerRole = AWS::IAM::Role {RoleName: {Ref: RoleName}, AssumeRolePolicyDocument trusting scheduler.amazonaws.com for sts:AssumeRole, inline policy allowing sqs:SendMessage on {Fn::GetAtt: [TargetQueue, Arn]}}; MySchedule = AWS::Scheduler::Schedule {Name: {Ref: ScheduleName}, GroupName: {Ref: MyGroup}, ScheduleExpression: 'rate(1 day)', State: 'DISABLED', FlexibleTimeWindow: {Mode: 'OFF'}, Target: {Arn: {Fn::GetAtt: [TargetQueue, Arn]}, RoleArn: {Fn::GetAtt: [SchedulerRole, Arn]}, Input: 'unused'}}.
2. cloudformation.create_stack(StackName=<s>, TemplateBody=<template>, Parameters=[GroupName, ScheduleName, QueueName, RoleName], Capabilities=['CAPABILITY_NAMED_IAM'], OnFailure='DELETE').
3. Wait on the stack_create_complete waiter (Delay 10, MaxAttempts 60) — it returns successfully.
4. Page scheduler.get_paginator('list_schedule_groups').paginate(NamePrefix=<GroupName>) and collect ScheduleGroups: no entry has Name == <GroupName> with State == 'ACTIVE' (AWS returns one).
5. scheduler.list_schedules(GroupName=<GroupName>) likewise does not contain <ScheduleName> with GroupName == <GroupName>.
