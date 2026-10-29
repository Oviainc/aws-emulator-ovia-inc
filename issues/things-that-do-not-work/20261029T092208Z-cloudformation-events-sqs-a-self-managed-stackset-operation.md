Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events__sqs.xsvc-cloudformation-2-stackset-operation-status
Issue-Key: cloudformation__events__sqs.xsvc-cloudformation-2-stackset-operation-status|_receive#16|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5,
Issue-Commit: f60279ee1b26e62c04ba7bfaecc753f1127e63e1

# cloudformation__events__sqs: a SELF_MANAGED StackSet operation never reaches a terminal status and none of the three CloudFormation StackSet status-change event types is published to the default bus, so the StackSet eventing check never grades

> _Note: this issue was identified against the code base at commit `f60279ee1b26e62c04ba7bfaecc753f1127e63e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStackInstances

## Description
Cause 1 of 1: the StackSet orchestration path neither drives a stack-set operation to a terminal status nor publishes the StackSet lifecycle events that must accompany it onto the default event bus, so a rule targeting SQS receives nothing and the check is killed at the harness ceiling. Failing check id: cloudformation__events__sqs.xsvc-cloudformation-2-stackset-operation-status.

AWS: recorded behavior only — the Smithy model describes CreateStackSet / CreateStackInstances / DescribeStackSetOperation / ListStackInstances / DeleteStackSet but says nothing about the EventBridge events, so the requirement is the recorded event shape. A SELF_MANAGED StackSet lifecycle emits, on the default bus, with source "aws.cloudformation", account and region equal to the caller's, and resources == [the stack-set ARN]:
  - detail-type "CloudFormation StackSet Status Change": detail.stack-set-arn, detail.status-details.status = ACTIVE after CreateStackSet and DELETED after DeleteStackSet;
  - detail-type "CloudFormation StackSet Operation Status Change": detail.stack-set-arn, detail.stack-set-operation-id = the OperationId the API call returned, detail.action = CREATE or DELETE, detail.status-details.status going RUNNING then SUCCEEDED (the RUNNING event's own `time` is not later than the SUCCEEDED event's);
  - detail-type "CloudFormation StackSet StackInstance Status Change": detail.stack-set-arn, detail.stack-id = the instance stack's id from ListStackInstances, detail.status-details.{status, detailed-status, status-reason}, with status CURRENT / detailed-status SUCCEEDED for an instance that deployed.
In all of these the fixed text is the three detail-type strings, the source "aws.cloudformation", the member names and the status literals (ACTIVE, DELETED, RUNNING, SUCCEEDED, CREATE, DELETE, CURRENT); the variable parts are the stack-set ARN, the StackSetId, the OperationId, the instance stack id, the account and the region, all taken from the request and the emulator's own state. The account id, region, generated set/role/queue names and timestamps in any one recording are per-run values, not requirements. Alongside the events, DescribeStackSetOperation must leave RUNNING/QUEUED/STOPPING for a terminal status so the operation can be observed to finish, and the deployed instance's stack must reach CREATE_COMPLETE with its AWS::SSM::Parameter readable.

Emulator: the check obtains no verdict at all — it is killed at the harness ceiling ("Check timed out after 60.0s") while polling DescribeStackSetOperation for a terminal status and draining its SQS target. Within that window the stack-set operation does not settle and not one of the three detail-types is delivered to either wired queue (the CONTROL rule matches all three detail-types on the set's ARN prefix; the CLAIM rule matches only status DELETED), so neither the ACTIVE event that would prove the pipe nor the RUNNING/SUCCEEDED operation pair nor the StackInstance event ever arrives.

Why: a client that gates on StackSet progress through EventBridge never fires — pipelines waiting for "Operation Status Change" SUCCEEDED, alarms on FAILED, and instance-level failure reasons all stay silent — and a client polling DescribeStackSetOperation instead gets no terminal status either, so a finished rollout is indistinguishable from one still running and from one that failed.

Fix: the StackSet operation orchestrator in the cloudformation crate (terminal status transitions for stack-set operations and for stack instances, and the instance stack's own deployment to CREATE_COMPLETE), together with its publication of the "CloudFormation StackSet Status Change", "CloudFormation StackSet Operation Status Change" and "CloudFormation StackSet StackInstance Status Change" events onto the default event bus.

Related already-open issue (does not carry this run's replay key, so this failure is not covered by it): 20261026T235900Z-cloudformation-events-sqs-stackset-operations-never-settle-and.md names this same check in its trace ids, but its recorded Issue-Key for it is `_receive#17|Timeout` and this run's failure is reported as a bare harness timeout, so the replay report lists this check as neither OWNED nor ALREADY FILED while it does mark its sibling cloudformation__events__sqs.xsvc-cloudformation-2-stackset-failed-instance-operation-status as OWNED by that file. A fixer closing that issue should close this one with it.

Traces: cloudformation__events__sqs.xsvc-cloudformation-2-stackset-operation-status

## Evidence
Failing check cloudformation__events__sqs.xsvc-cloudformation-2-stackset-operation-status — verdict [mismatch]:

  Error: Check timed out after 60.0s

The check's own budgets are far larger than the ceiling it hit: OP_TIMEOUT_S = 150.0 for DescribeStackSetOperation to reach a terminal status and DELIVERY_TIMEOUT_S = 90.0 per drain, so the 60 s harness kill means neither the operation nor any delivery had completed.

Expected (AWS), asserted by the check in order:
  1. After CreateStackSet(PermissionModel='SELF_MANAGED', AdministrationRoleARN=<admin>, ExecutionRoleName=<exec>), a "CloudFormation StackSet Status Change" event with detail.status-details.status == 'ACTIVE', source 'aws.cloudformation', account == caller account, region == caller region, resources == [arn:aws:cloudformation:<region>:<account>:stackset/<StackSetId>] and detail.stack-set-arn equal to that same ARN.
  2. After CreateStackInstances(Accounts=[account], Regions=[region], OperationPreferences={FailureToleranceCount:0, MaxConcurrentCount:1}), DescribeStackSetOperation reaching Status 'SUCCEEDED', ListStackInstances returning exactly 1 summary, and "CloudFormation StackSet Operation Status Change" events with detail.stack-set-operation-id == the returned OperationId, detail.action == 'CREATE' and detail.status-details.status 'RUNNING' then 'SUCCEEDED' (RUNNING['time'] <= SUCCEEDED['time']), plus a "CloudFormation StackSet StackInstance Status Change" event whose detail.stack-id is the instance stack id, detail.status-details.detailed-status == 'SUCCEEDED' and detail.status-details.status == 'CURRENT'.
  3. DescribeStacks(<instance stack>) StackStatus == 'CREATE_COMPLETE' and the deployed AWS::SSM::Parameter readable with value 'sim2real'.
  4. After DeleteStackInstances, an Operation Status Change with detail.action == 'DELETE' and status 'SUCCEEDED'; after DeleteStackSet, the CLAIM queue holding exactly one event, a "CloudFormation StackSet Status Change" with status 'DELETED', and ACTIVE['time'] <= DELETED['time'].

Actual (emulator): no terminal stack-set operation status and no event of any of the three detail-types inside the window — the check is terminated at 60 s before reaching assertion 1's event, so the whole StackSet eventing contract is unobservable.

Setup the emulator accepted: both wired rules were created and PutTargets answered FailedEntryCount 0 for each (asserted by the check's _wired_queue helper), and each SQS queue carried a policy allowing events.amazonaws.com sqs:SendMessage conditioned on aws:SourceArn == the rule ARN, so the rule/target wiring is not the blocker.

## Steps to Reproduce
1. Boot the emulator and create cloudformation, events, sqs, iam, ssm and sts clients.
2. Create two SQS queues, each with a policy allowing events.amazonaws.com to sqs:SendMessage conditioned on the rule ARN. Create two EventBridge rules on the default bus targeting them:
   - CONTROL: {"source": ["aws.cloudformation"], "detail-type": ["CloudFormation StackSet Status Change", "CloudFormation StackSet Operation Status Change", "CloudFormation StackSet StackInstance Status Change"], "detail": {"stack-set-arn": [{"prefix": "arn:aws:cloudformation:<region>:<account>:stackset/<set name>:"}]}}
   - CLAIM: the same prefix but only detail-type "CloudFormation StackSet Status Change" and detail.status-details.status ["DELETED"].
   Confirm PutTargets answers FailedEntryCount 0 for both.
3. Create an administration role trusted by cloudformation.amazonaws.com that may sts:AssumeRole the execution role, and an execution role trusting the administration role.
4. CreateStackSet(StackSetName=<name>, TemplateBody with one AWS::SSM::Parameter, PermissionModel='SELF_MANAGED', AdministrationRoleARN=<admin arn>, ExecutionRoleName=<exec name>). Drain the CONTROL queue and expect a "CloudFormation StackSet Status Change" event with detail.status-details.status 'ACTIVE'.
5. CreateStackInstances(Accounts=[caller account], Regions=[caller region]); poll DescribeStackSetOperation for a terminal Status and drain CONTROL. Expect Status 'SUCCEEDED' and the Operation Status Change RUNNING->SUCCEEDED pair carrying the returned OperationId and action 'CREATE', plus the StackInstance Status Change event for the instance stack id from ListStackInstances.
6. DescribeStacks on the instance stack and GetParameter on the deployed parameter; expect CREATE_COMPLETE and value 'sim2real'.
7. DeleteStackInstances, then DeleteStackSet; expect the DELETE Operation Status Change and exactly one 'DELETED' StackSet Status Change on the CLAIM queue.
8. Observed: the operation never settles and no event arrives; the check is killed at the 60 s harness ceiling.
9. Re-replay cloudformation__events__sqs.xsvc-cloudformation-2-stackset-operation-status to confirm the fix.
