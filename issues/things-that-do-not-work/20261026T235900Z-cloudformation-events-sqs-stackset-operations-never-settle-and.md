Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events__sqs.xsvc-cloudformation-2-stackset-operation-status cloudformation__events__sqs.xsvc-cloudformation-2-stackset-failed-instance-operation-status
Issue-Key: cloudformation__events__sqs.xsvc-cloudformation-2-stackset-operation-status|_receive#17|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5,
Issue-Key: cloudformation__events__sqs.xsvc-cloudformation-2-stackset-failed-instance-operation-status|_receive#15|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5,
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# cloudformation__events__sqs: StackSet operations never settle and their EventBridge StackSet/Operation/StackInstance status-change events never arrive, so both StackSet eventing checks exhaust the replay window

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStackInstances

## Description
Cause 1 of 1 — the StackSet orchestrator produces neither a terminal operation status nor the default-bus status-change events that must accompany it.
AWS: recorded behavior only (field names read off real us-west-2 events) — a SELF_MANAGED StackSet lifecycle emits, on the default event bus: `CloudFormation StackSet Status Change` (detail.stack-set-arn, detail.status-details.status = ACTIVE on CreateStackSet, DELETED on DeleteStackSet); `CloudFormation StackSet Operation Status Change` (detail.stack-set-arn, detail.stack-set-operation-id, detail.action CREATE|DELETE, detail.status-details.status RUNNING then SUCCEEDED, or RUNNING then FAILED when the instance's own stack fails); and `CloudFormation StackSet StackInstance Status Change` (detail.stack-set-arn, detail.stack-id, detail.status-details.{status, detailed-status, status-reason}, whose status-reason equals ListStackInstances' StatusReason and names the failing logical resource). The joins are to the runtime ids the API calls returned (OperationId, StackSetId, the instance stack's id), and the operation reaches a terminal status so those events can be emitted.
Emulator: neither check obtains a verdict — both are killed at the harness ceiling (60 s) while polling for the operation's terminal status and for pattern-matched deliveries into their SQS target, i.e. the StackSet operation never settles and no StackSet/Operation/StackInstance event is delivered. The earlier convergence issue 20261025T015306Z-cloudformation-long-running-operations-never-converge-23-behavioral.md is marked FIXED and does not cover these failures.
Why: anything that reacts to StackSet progress through EventBridge (pipelines gating on SUCCEEDED, alarms on FAILED, instance-level failure reasons) never fires, and a caller polling the operation gets no terminal status either, so a failed instance is indistinguishable from one still rolling out.
Hypothesis: one root cause with two faces — the StackSet orchestration stage that must drive CreateStackInstances/DeleteStackSet operations to a terminal status is also the stage that would publish the three detail-types, so no event is emitted because no status transition completes. The replay output states only the timeout, not which stage stalls.
Fix: the StackSet operation orchestrator in the cloudformation crate (terminal status transitions for stack-set operations and stack instances) and its EventBridge publication of the StackSet / StackSet Operation / StackSet StackInstance status-change events onto the default bus.
Traces: cloudformation__events__sqs.xsvc-cloudformation-2-stackset-operation-status, cloudformation__events__sqs.xsvc-cloudformation-2-stackset-failed-instance-operation-status

## Evidence
cloudformation__events__sqs.xsvc-cloudformation-2-stackset-operation-status:
  Error: Check timed out after 60.0s   (OP_TIMEOUT_S 150.0, DELIVERY_TIMEOUT_S 90.0; asserts StackSet Status Change ACTIVE/DELETED, Operation Status Change RUNNING->SUCCEEDED with the OperationId CreateStackInstances returned, and StackInstance Status Change carrying the instance stack's id, ordered on each event's own `time`)

cloudformation__events__sqs.xsvc-cloudformation-2-stackset-failed-instance-operation-status:
  Error: Check timed out after 60.0s   (OP_TIMEOUT_S 240.0; the instance's stack fails on an unsignalled WaitCondition with Timeout 1 and the operation must report FAILED, never SUCCEEDED, with StackInstance Status Change status OUTDATED / detailed-status FAILED and a status-reason equal to ListStackInstances' StatusReason naming the failing logical resource)

## Steps to Reproduce
1. Create an EventBridge rule on the default bus matching source aws.cloudformation detail-types 'CloudFormation StackSet Status Change', 'CloudFormation StackSet Operation Status Change', 'CloudFormation StackSet StackInstance Status Change', targeting an SQS queue.
2. CreateStackSet (SELF_MANAGED, admin+exec roles), CreateStackInstances for the caller account/Region, then poll DescribeStackSetOperation for a terminal status and drain the queue.
3. Expect the operation to reach SUCCEEDED (and, with a template whose WaitCondition is never signalled, FAILED) and the three detail-types to arrive carrying the returned OperationId/StackSetId/stack id; the emulator produces no terminal operation status and no deliveries inside the replay window.
4. DeleteStackSet and expect the StackSet Status Change DELETED event.
