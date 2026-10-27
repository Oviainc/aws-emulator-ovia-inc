Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back
Issue-Key: cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back|run#11|AssertionError|assert "CREATE_FAILED" in events, f"the vector bucket's events are {events!r}"
Issue-Commit: 67bdaa95f371a43447a945b4dd8bda66772a6b0f

# CloudFormation records no per-resource stack events: a rolled-back stack's failing resource has no CREATE_FAILED event in DescribeStackEvents

> _Note: this issue was identified against the code base at commit `67bdaa95f371a43447a945b4dd8bda66772a6b0f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
One root cause.

Service attribution note: the failing check's id token is the multi-service group `cloudformation__s3__s3tables__s3vectors`, whose SERVICES are [cloudformation, s3, s3tables, s3vectors]. The assertion that fails is on CloudFormation's own event log (DescribeStackEvents), reached AFTER the stack has already settled, so the diverging service is `cloudformation` (bootstrapped). s3tables is NOT bootstrapped, but nothing in this check reaches s3tables before the failure.

Related open issues that do NOT carry this replay key and do not cover it: 20261025T015306Z-cloudformation-stack-events-and-resource-views-are-incomplete.md (its Cause 1 describes the same missing per-resource event path but is grounded in other checks — this check's key is not in its Issue-Key list), and 20261023T014718Z-cloudformation-s3-s3tables-s3vectors-cloudformation-does-not-propagate-an-s3-vectors-resource.md (grounded in the earlier `status == ROLLBACK_COMPLETE` assertion of the same check, which now PASSES; the failure has moved past it to the event log).

Cause 1 of 1: a stack whose resource fails provisioning rolls back, but no event is ever recorded against the failing resource's LogicalResourceId.
AWS: Smithy com.amazonaws.cloudformation#StackEvent carries LogicalResourceId, PhysicalResourceId, ResourceType, ResourceStatus (enum, includes CREATE_FAILED) and ResourceStatusReason; DescribeStackEvents returns the events for a stack's operations. Every declared resource transitions through at least one IN_PROGRESS and one terminal resource status, and the resource whose provisioning was refused is recorded as CREATE_FAILED with the refusal in ResourceStatusReason — that event is how a client learns WHICH resource and WHICH property caused the rollback. The StackEvent doc for DetailedStatus=VALIDATION_FAILED explicitly says ResourceStatusReason shows the incorrectly defined properties.
Emulator: the stack correctly settles at ROLLBACK_COMPLETE, but DescribeStackEvents returns no event at all whose LogicalResourceId is the failing resource — `AssertionError: the vector bucket's events are []`. Only stack-level (AWS::CloudFormation::Stack) events appear to be recorded, so the resource-scoped slice is empty.
Why: a client that deploys a stack and sees ROLLBACK_COMPLETE has no way to find out what went wrong: the event log is the only audit trail of a deploy, and the CREATE_FAILED event plus its ResourceStatusReason is the only place the provider's refusal surfaces. CLI/console flows (`aws cloudformation describe-stack-events` after a failed deploy), CDK/SAM deploy failure reporting and any CI gate that greps the event log for the failing logical id all read an empty history and report a failure with no cause.
Fix: the CloudFormation provisioning engine must append a per-resource StackEvent for each resource transition (at minimum CREATE_IN_PROGRESS and the terminal CREATE_COMPLETE / CREATE_FAILED, plus DELETE_* during rollback), populating LogicalResourceId, ResourceType, ResourceStatus and ResourceStatusReason, and the DescribeStackEvents handler must serve them alongside the stack-level events.
Traces: cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back.

## Evidence
Check: cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back [mismatch].

Two stacks are created: one with a valid AWS::S3Vectors::VectorBucket name, one whose VectorBucketName is 'Upper_Case_Vec' (refused by S3 Vectors' lower-case name rule).

Passing now: the valid stack reaches CREATE_COMPLETE and the invalid stack reaches ROLLBACK_COMPLETE.

Expected: `[e['ResourceStatus'] for e in describe_stack_events(StackName=bad)['StackEvents'] if e['LogicalResourceId'] == 'Vectors']` contains 'CREATE_FAILED' (AWS also carries the provider refusal in that event's ResourceStatusReason).
Actual: `AssertionError: the vector bucket's events are []` — the resource-scoped event list is empty; no event names the logical id 'Vectors' at all.

(File/line: check_cloudformation__s3__s3tables__s3vectors_cfn_s3vectors_invalid_vector_bucket_name_rejected_and_stack_rolls_back.py line 44.)

## Steps to Reproduce
1. Boot the emulator and replay cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back.
2. Or by hand: CreateStack with a template whose only resource is logical id 'Vectors' of type AWS::S3Vectors::VectorBucket with VectorBucketName='Upper_Case_Vec' (a name S3 Vectors refuses).
3. Poll DescribeStacks until the stack leaves *_IN_PROGRESS — it correctly reports ROLLBACK_COMPLETE.
4. Call DescribeStackEvents(StackName=<stack>) and filter StackEvents on LogicalResourceId == 'Vectors'. AWS returns at least a CREATE_FAILED event (with ResourceStatusReason naming the refused property); the emulator returns an empty list, with only AWS::CloudFormation::Stack-level events present.
5. Repeat with a stack that succeeds: no CREATE_IN_PROGRESS/CREATE_COMPLETE event is recorded for the resource logical id either, confirming the event path is resource-blind rather than rollback-specific.
