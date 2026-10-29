Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events__sqs.cfn-resource-status-change-event-carries-the-runtime-physical-id cloudformation__events__sqs.xsvc-cloudformation-1-delete cloudformation__events__sqs.xsvc-cloudformation-1-pattern-precision cloudformation__events__sqs.xsvc-cloudformation-2-drift-detection-status
Issue-Key: cloudformation__events__sqs.cfn-resource-status-change-event-carries-the-runtime-physical-id|_receive#27|Timeout|resp = sqs.receive_message(
Issue-Key: cloudformation__events__sqs.xsvc-cloudformation-1-delete|_drain_until#24|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2, VisibilityTimeout=60)
Issue-Key: cloudformation__events__sqs.xsvc-cloudformation-1-pattern-precision|_receive_into#18|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=wait_s, VisibilityTimeout=60)
Issue-Key: cloudformation__events__sqs.xsvc-cloudformation-2-drift-detection-status|_receive#19|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5,
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# CloudFormation publishes no stack/resource status-change or drift-detection events to EventBridge (2 causes)

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Two root causes. The open issues 20261026T235900Z-cloudformation-events-sqs-stackset-operations-never-settle-and.md and 20261029T092208Z-cloudformation-events-sqs-a-self-managed-stackset-operation.md cover the StackSet variants only; the checks below were not bound to them.

Cause 1 of 2: no "CloudFormation Stack Status Change" / "CloudFormation Resource Status Change" event reaches EventBridge.
AWS: CloudFormation emits events on the default bus with source "aws.cloudformation" and detail-type "CloudFormation Stack Status Change" and "CloudFormation Resource Status Change" as a stack and its resources move through CREATE_IN_PROGRESS/CREATE_COMPLETE/DELETE_IN_PROGRESS/DELETE_COMPLETE. The resource-status detail carries stack-id, logical-resource-id, physical-resource-id, status-details.status and resource-type; the envelope carries version/id/detail-type/source/account/time/region/resources with the stack ARN in resources. recorded behavior only for the exact detail body.
Emulator: a CreateStack followed by DeleteStack yields nothing on a rule matching source aws.cloudformation; the consumer poll ends "Check timed out after 60.0s". The pattern-precision check cannot be graded because its positive control never arrives either.
Why: a client that waits on stack lifecycle through EventBridge (deployment pipelines, drift remediation, physical-id capture) hangs forever instead of being driven by the stack it just created.
Fix: the CloudFormation crate's stack and resource state-transition path must publish to the EventBridge bus; the physical-resource-id in the resource event must be the runtime id the provider minted.
Traces: cloudformation__events__sqs.cfn-resource-status-change-event-carries-the-runtime-physical-id, cloudformation__events__sqs.xsvc-cloudformation-1-delete, cloudformation__events__sqs.xsvc-cloudformation-1-pattern-precision

Cause 2 of 2: no "CloudFormation Drift Detection Status Change" event is published.
AWS: DetectStackDrift drives a detection operation that, on completion, emits source "aws.cloudformation", detail-type "CloudFormation Drift Detection Status Change" with detail carrying stack-id, status-details.detection-status and stack-drift-status. recorded behavior only for the detail body.
Emulator: the rule's queue stays empty; "Check timed out after 60.0s".
Why: drift-driven automation never fires.
Fix: the drift-detection operation handler in the CloudFormation crate, plus the same event-emission path as cause 1.
Traces: cloudformation__events__sqs.xsvc-cloudformation-2-drift-detection-status

## Evidence
All four checks end "Error: Check timed out after 60.0s" with an empty SQS target behind an EventBridge rule whose pattern is {"source":["aws.cloudformation"]}. Expected: at least one envelope per stack/resource/drift transition, each with the documented detail-type and a detail body naming the stack and resource.

## Steps to Reproduce
PutRule on {"source":["aws.cloudformation"]} with an SQS target and a permissive queue policy, then CreateStack and DeleteStack a trivial stack (cause 1) or DetectStackDrift on it (cause 2), and poll the queue. Nothing arrives within the run budget.
