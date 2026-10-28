Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler.deploy-cfn-schedule-group-with-nested-schedule scheduler.deploy-cfn-schedule-invokes-sqs-target scheduler.deploy-cloudformation-schedule-group-membership-behaves scheduler.deploy-cfn-schedule-update-changes-target-behavior scheduler.deploy-cloudformation-schedule-invokes-target lambda__scheduler.cfn-deploy-scheduler-schedule-group-positive lambda__scheduler.cfn-deploy-scheduler-lambda-target-positive lambda__scheduler.cfn-deploy-scheduler-role-trust-policy-positive lambda__scheduler.cfn-deploy-scheduler-invalid-target-arn-negative lambda__scheduler.cfn-deploy-scheduler-missing-invoke-permission-negative lambda__scheduler.cfn-stack-update-scheduler-cron-expression-positive
Issue-Key: scheduler.deploy-cfn-schedule-group-with-nested-schedule|run#14|AssertionError|assert any(g["Name"] == group_name and g["State"] == "ACTIVE" for g in groups), (
Issue-Key: scheduler.deploy-cloudformation-schedule-group-membership-behaves|_deploy_cfn#7|WaiterError|waiter.wait(StackName=stack_name, WaiterConfig={"Delay": 10, "MaxAttempts": 60})
Issue-Key: scheduler.deploy-cfn-schedule-update-changes-target-behavior|_poll_for_message#16|Timeout|resp = sqs.receive_message(
Issue-Key: scheduler.deploy-cloudformation-schedule-invokes-target|_poll_for_message#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=10)
Issue-Key: lambda__scheduler.cfn-deploy-scheduler-lambda-target-positive|_drain_for_message#10|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__scheduler.cfn-deploy-scheduler-role-trust-policy-positive|_drain_for_message#16|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__scheduler.cfn-deploy-scheduler-invalid-target-arn-negative|_drain_for_message#15|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__scheduler.cfn-deploy-scheduler-missing-invoke-permission-negative|_drain_for_message#13|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__scheduler.cfn-stack-update-scheduler-cron-expression-positive|_drain_for_tag#15|Timeout|resp = sqs.receive_message(
Issue-Related: 20261023T191008Z-scheduler-control-plane-ten-divergences-in-createschedule-updateschedule.md
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# CloudFormation stacks containing AWS::Scheduler::Schedule / AWS::Scheduler::ScheduleGroup do not create the resources (or fail the stack outright)

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Single root cause (1 of 1): the CloudFormation path does not provision the scheduler resource types. Two symptoms of the same gap are observed.

AWS: a stack declaring `AWS::Scheduler::ScheduleGroup` and `AWS::Scheduler::Schedule` reaches CREATE_COMPLETE only after the resources exist in EventBridge Scheduler: ListScheduleGroups/GetScheduleGroup then report the group ACTIVE, ListSchedules(GroupName=...) reports the nested schedule with GroupName set and its declared State, GetSchedule(Name=..., GroupName=...) resolves it, and DeleteStack removes both (recorded behavior; the scheduler Smithy model's CreateSchedule/CreateScheduleGroup are the underlying operations the resource types call, with GroupName len 1..64 pattern ^[0-9a-zA-Z-_.]+$).

Emulator: (a) the stack reports CREATE_COMPLETE while neither resource exists — ListScheduleGroups does not show the group, and GetSchedule for the nested schedule answers HTTP 404 ResourceNotFoundException ('Schedule <name> does not exist in schedule group <group>'); (b) an equivalent template (same two resource types plus AWS::SQS::Queue and an unnamed AWS::IAM::Role) instead drives the stack to CREATE_FAILED, so the waiter raises a terminal failure. Either way no scheduler resource is ever created by CloudFormation, and the stack status does not reflect reality.

Why: everything deployed as IaC is silently absent. A client that owns its schedules through CloudFormation gets a green stack and no schedules, so every subsequent read 404s and every downstream expectation (group membership, firing, update-in-place through a stack update) is unreachable; the CREATE_FAILED variant blocks the deployment entirely.

Fix: the CloudFormation resource-provider mapping for AWS::Scheduler::ScheduleGroup and AWS::Scheduler::Schedule — the resource dispatch table in the CloudFormation service crate (services/service-com-amazonaws-cloudformation-cloudformation/src/) must call this service's CreateScheduleGroup/CreateSchedule (services/service-com-amazonaws-scheduler-awschronosservice/src/ops/create_schedule_group.rs, create_schedule.rs) on create and the matching deletes on stack delete, and must report CREATE_FAILED only when that call actually fails.

Traces: the check ids in trace_ids. scheduler.deploy-cfn-schedule-group-with-nested-schedule and lambda__scheduler.cfn-deploy-scheduler-schedule-group-positive show symptom (a); scheduler.deploy-cloudformation-schedule-group-membership-behaves shows symptom (b). The remaining CFN-based checks listed time out because they wait on a schedule the stack never created (and, for the lambda-target ones, additionally need Lambda Invoke, owned by 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md).

## Evidence
Symptom (a), scheduler.deploy-cfn-schedule-group-with-nested-schedule: waiter stack_create_complete succeeds, then AssertionError: 'schedule group sim2real-sched-grp-4d23e8dfd11d not visible via ListScheduleGroups'. Same shape in scheduler.deploy-cfn-schedule-invokes-sqs-target and lambda__scheduler.cfn-deploy-scheduler-schedule-group-positive: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetSchedule operation: Schedule sim2real-xsvc-sched-98609bbac9be does not exist in schedule group sim2real-xsvc-grp-7bbbfe9d7766.
Symptom (b), scheduler.deploy-cloudformation-schedule-group-membership-behaves: botocore.exceptions.WaiterError: Waiter StackCreateComplete failed: Waiter encountered a terminal failure state: For expression "Stacks[].StackStatus" we matched expected path: "CREATE_FAILED" at least once.
Expected in both: stack CREATE_COMPLETE and the group/schedule readable through ListScheduleGroups / ListSchedules / GetSchedule, removed again by DeleteStack.

## Steps to Reproduce
1. Build a template with Resources: {ScheduleGroup: AWS::Scheduler::ScheduleGroup {Name: <g>}, Queue: AWS::SQS::Queue, SchedulerRole: AWS::IAM::Role trusting scheduler.amazonaws.com, Schedule: AWS::Scheduler::Schedule {Name: <s>, GroupName: {Ref: ScheduleGroup}, ScheduleExpression: 'rate(1 day)', State: 'DISABLED', FlexibleTimeWindow: {Mode: OFF}, Target: {Arn: {Fn::GetAtt: [Queue, Arn]}, RoleArn: {Fn::GetAtt: [SchedulerRole, Arn]}, Input: 'unused'}}}.
2. cloudformation.create_stack(..., Capabilities=['CAPABILITY_NAMED_IAM']) and wait for stack_create_complete.
3. scheduler.list_schedule_groups() -> <g> absent (or the waiter fails with CREATE_FAILED for the variant with an unnamed IAM role).
4. scheduler.get_schedule(Name=<s>, GroupName=<g>) -> 404 ResourceNotFoundException instead of the schedule the stack declared.
