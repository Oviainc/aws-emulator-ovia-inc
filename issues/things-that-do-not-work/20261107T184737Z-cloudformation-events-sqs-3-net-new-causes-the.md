Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events__sqs.xsvc-cloudformation-1-delete cloudformation__events__sqs.xsvc-cloudformation-1-pattern-precision cloudformation__events__sqs.xsvc-cloudformation-2-drift-detection-status cloudformation__events__sqs.xsvc-cloudformation-2-stackset-operation-status cloudformation__events__sqs.xsvc-cloudformation-2-stackset-failed-instance-operation-status
Issue-Key: cloudformation__events__sqs.xsvc-cloudformation-1-delete|_drain_until#24|AssertionError|raise AssertionError(
Issue-Key: cloudformation__events__sqs.xsvc-cloudformation-1-pattern-precision|_drain_until#18|AssertionError|raise AssertionError(f"queue never yielded [{why}] within {DELIVERY_TIMEOUT_S}s; got {[_brief(e) for e in seen][:12]!r}")
Issue-Key: cloudformation__events__sqs.xsvc-cloudformation-2-drift-detection-status|_drain_until#19|AssertionError|raise AssertionError(
Issue-Key: cloudformation__events__sqs.xsvc-cloudformation-2-stackset-operation-status|_drain_until#16|AssertionError|raise AssertionError(
Issue-Key: cloudformation__events__sqs.xsvc-cloudformation-2-stackset-failed-instance-operation-status|_drain_until#15|AssertionError|raise AssertionError(
Issue-Commit: 8662809dae52673eae9c053e761f1b93b4d6fc67

# cloudformation__events__sqs: 3 net-new causes — the delete/create-failure stack and resource status events, the drift-detection events, and the whole StackSet event family are never published to EventBridge

> _Note: this issue was identified against the code base at commit `8662809dae52673eae9c053e761f1b93b4d6fc67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStack / CreateStack / DetectStackDrift / CreateStackSet+CreateStackInstances (EventBridge default-bus publication)

## Description
This issue carries 3 net-new root causes. All three are "CloudFormation publishes no event", but they are three distinct event families emitted by three distinct code paths, and each has its own detail-type and detail shape.

Cause 1 of 3: the stack and per-resource status-change events of the DELETE path and of a failing CREATE are never published, so a rule on the default bus matching those stacks receives nothing.
Failing check ids for this cause: cloudformation__events__sqs.xsvc-cloudformation-1-delete, cloudformation__events__sqs.xsvc-cloudformation-1-pattern-precision.
AWS: recorded behavior only — EventBridge envelopes are not described by the CloudFormation Smithy model. The recording shows DeleteStack with a ClientRequestToken delivering, on the default bus, detail-type "CloudFormation Stack Status Change" for DELETE_IN_PROGRESS (status-reason "User Initiated") and DELETE_COMPLETE, plus detail-type "CloudFormation Resource Status Change" DELETE_IN_PROGRESS/DELETE_COMPLETE for each resource, every event carrying the delete's client-request-token and the physical id the resource held before deletion (arrival order at the queue is not guaranteed). It also shows a failing create delivering roughly fifteen events to a rule matching both detail-types, including the resource's CREATE_FAILED and the stack's ROLLBACK_COMPLETE.
Emulator: nothing arrives. xsvc-cloudformation-1-delete fails with "queue never yielded [stack and per-resource DELETE_IN_PROGRESS/DELETE_COMPLETE under the delete token] within 120.0s; got []" and xsvc-cloudformation-1-pattern-precision fails on its own positive control: "queue never yielded [positive control: main rule receives CREATE_FAILED and ROLLBACK_COMPLETE] within 120.0s; got []". Because the positive control is empty, the near-miss rules that must stay quiet prove nothing.
Why: nothing can react to a stack's lifecycle. Delete-completion automation, rollback alerting and any pattern scoped to a stack id or a status never fire, and a consumer cannot tell a correctly-scoped rule from a bus on which CloudFormation is silent.
Fix: the CloudFormation stack-operation and per-resource provisioning paths must publish "CloudFormation Stack Status Change" and "CloudFormation Resource Status Change" events to the default EventBridge bus for the delete and rollback transitions as they already must for create, carrying the operation's client-request-token, the stack id/name, the logical and physical resource id and the status (and status-reason where AWS sets one).

Cause 2 of 3: DetectStackDrift publishes no "CloudFormation Drift Detection Status Change" events.
Failing check ids for this cause: cloudformation__events__sqs.xsvc-cloudformation-2-drift-detection-status.
AWS: recorded behavior only. The recording shows one detection emitting two events of detail-type "CloudFormation Drift Detection Status Change" — DETECTION_IN_PROGRESS (with an empty stack-drift-status and a drifted-resource count of -1) and DETECTION_COMPLETE — both carrying the stack id and the detection id, with no guaranteed arrival order.
Emulator: the control rule's queue stays empty: "queue never yielded [IN_PROGRESS + COMPLETE events for detection '<detection-id>'] within 90.0s; got 0 events: []" (the detection id is per-run and not part of the requirement). Neither event is published, so a rule scoped to DRIFTED also proves nothing.
Why: drift detection cannot be observed asynchronously — compliance automation that waits for DETECTION_COMPLETE, or that routes only DRIFTED outcomes, never runs.
Fix: the DetectStackDrift/DetectStackResourceDrift path must publish the DETECTION_IN_PROGRESS and DETECTION_COMPLETE "CloudFormation Drift Detection Status Change" events, carrying the stack id, the detection id, the stack drift status and the drifted-resource count.

Cause 3 of 3: no StackSet event is ever published — neither the stack-set's own status change, nor its operations' status changes, nor its stack instances' status changes.
Failing check ids for this cause: cloudformation__events__sqs.xsvc-cloudformation-2-stackset-operation-status, cloudformation__events__sqs.xsvc-cloudformation-2-stackset-failed-instance-operation-status.
AWS: recorded behavior only. The recording shows CreateStackSet delivering detail-type "CloudFormation StackSet Status Change" with detail.stack-set-arn and detail.status-details.status ACTIVE (DELETED on DeleteStackSet); each operation delivering "CloudFormation StackSet Operation Status Change" with detail.stack-set-operation-id, detail.action (CREATE|DELETE) and detail.status-details.status RUNNING then SUCCEEDED (FAILED when the operation fails); and each instance delivering "CloudFormation StackSet StackInstance Status Change" with detail.stack-id once the stack exists and detail.status-details.{status, detailed-status, status-reason} — for a resource that fails to create, status OUTDATED with detailed-status FAILED and a status-reason equal to ListStackInstances' StatusReason, naming the failing logical resource as "ResourceLogicalId:<id>, ...".
Emulator: the control queue is empty from the very first event: "queue never yielded [StackSet Status Change ACTIVE after CreateStackSet] within 90.0s; got 0 events: []" and "queue never yielded [StackSet Status Change ACTIVE after CreateStackSet (pipe control)] within 90.0s; got 0 events: []". No event of any of the three StackSet detail-types is published, so neither the success ordering nor the FAILED-instance routing can be observed.
Why: StackSet rollouts are unobservable. Operators and automation cannot be notified that a stack set became ACTIVE, that an operation finished, or that a particular instance failed and why — the per-instance status-reason is the only place the failing logical resource is named.
Fix: the StackSet control path must publish the three StackSet event families to the default bus on stack-set, operation and stack-instance transitions, with the detail fields AWS emits (stack-set-arn, stack-set-operation-id, action, stack-id, status-details.status/detailed-status/status-reason).

Traces: the five check ids in trace_ids, grouped per cause above.

The open issue 20261029T182228Z-cloudformation-events-sqs-cloudformation-publishes-no-stack-resource-status-change-or.md carries the replay key of cfn-resource-status-change-event-carries-the-runtime-physical-id only; it carries none of the five keys above and none of these three event families' causes.

## Evidence
Cause 1 — Expected: DeleteStack with a ClientRequestToken delivers stack DELETE_IN_PROGRESS ("User Initiated") and DELETE_COMPLETE plus each resource's DELETE_IN_PROGRESS/DELETE_COMPLETE on the default bus, all carrying the token and the pre-deletion physical id; a failing create delivers CREATE_FAILED and ROLLBACK_COMPLETE to a rule on both detail-types. Actual: AssertionError: queue never yielded [stack and per-resource DELETE_IN_PROGRESS/DELETE_COMPLETE under the delete token] within 120.0s; got []  /  AssertionError: queue never yielded [positive control: main rule receives CREATE_FAILED and ROLLBACK_COMPLETE] within 120.0s; got [].

Cause 2 — Expected: two "CloudFormation Drift Detection Status Change" events (DETECTION_IN_PROGRESS, DETECTION_COMPLETE) carrying the stack id and detection id. Actual: AssertionError: queue never yielded [IN_PROGRESS + COMPLETE events for detection '<id>'] within 90.0s; got 0 events: [].

Cause 3 — Expected: "CloudFormation StackSet Status Change" status ACTIVE after CreateStackSet, then per-operation RUNNING/SUCCEEDED (or FAILED) and per-instance status/detailed-status/status-reason events. Actual: AssertionError: queue never yielded [StackSet Status Change ACTIVE after CreateStackSet] within 90.0s; got 0 events: []  (and the same for the "(pipe control)" variant).

## Steps to Reproduce
Cause 1: create an EventBridge rule on the default bus matching detail-types "CloudFormation Stack Status Change" and "CloudFormation Resource Status Change" for a stack-id prefix, target an SQS queue, CreateStack a two-resource stack, then DeleteStack with a ClientRequestToken and drain the queue. Repeat with a template whose resource fails to create and look for CREATE_FAILED/ROLLBACK_COMPLETE.
Cause 2: rule on detail-type "CloudFormation Drift Detection Status Change" -> SQS, then DetectStackDrift on a deployed stack and drain the queue.
Cause 3: rules on "CloudFormation StackSet Status Change", "...StackSet Operation Status Change" and "...StackSet StackInstance Status Change" -> SQS, then CreateStackSet + CreateStackInstances (once with a template that succeeds, once with an unsignalled WaitCondition with Timeout 1 so the instance fails) and drain the queues.
Or replay the five check ids in trace_ids.
