Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.create-stack-s3-bucket-name-conflict-rolls-back cloudformation.resource-schema-rejects-unknown-and-mistyped-properties
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.create-stack-s3-bucket-name-conflict-rolls-back|run#13|AssertionError|assert any("Validation failed" in r for r in reasons), (
Issue-Key: cloudformation.resource-schema-rejects-unknown-and-mistyped-properties|_assert_schema_failure#11|AssertionError|assert reason == SCHEMA_FAILURE_REASON, (
Issue-Commit: 35f0669fbed72946ad7349f4657cbc8fac9f7c2b

# CloudFormation emits no per-resource stack events, and resource-property (schema) validation is never performed (2 causes)

> _Note: this issue was identified against the code base at commit `35f0669fbed72946ad7349f4657cbc8fac9f7c2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
Two net-new root causes in the CloudFormation emulator, neither marked ALREADY FILED nor OWNED in the replay report. Both make a failed stack's cause unreadable to a client. Related open issue 20261025T015306Z-cloudformation-stack-events-and-resource-views-are-incomplete.md is about the content of event/resource views for other checks and does NOT carry either replay key below.

Cause 1 of 2: DescribeStackEvents returns stack-level events only — no event is ever recorded for a logical resource, and the resource-provider failure text is folded into a stack-level CREATE_FAILED reason.
AWS: Smithy (com.amazonaws.cloudformation) StackEvent.LogicalResourceId is documented as 'The logical name of the resource specified in the template' and StackEvent.ResourceStatusReason as 'Success/failure message associated with the resource'; ResourceStatus includes CREATE_IN_PROGRESS/CREATE_FAILED/CREATE_COMPLETE per resource. Real AWS therefore emits a CREATE_FAILED event whose LogicalResourceId is the failing resource and whose ResourceStatusReason is CloudFormation's own validation/handler reason (the recording for this check shows a reason containing 'Validation failed'), while the aggregate sentence 'The following resource(s) failed to create: [X].' belongs to the STACK-level event.
Emulator: for the rolled-back stack the only CREATE_FAILED event present is the stack-level one, and its reason is the emulator's aggregate text with the downstream service error appended — e.g. 'The following resource(s) failed to create: [Bucket]. BucketAlreadyOwnedByYou: Your previous request to create the named bucket succeeded and you already own it.' No event carries LogicalResourceId 'Bucket'.
Why: a client (and every CLI/CDK progress view) identifies which resource failed, and why, from the per-resource CREATE_FAILED event. With only a stack-level event it cannot attribute the failure to a resource, cannot distinguish a validation refusal from a throttle or a permissions failure, and cannot drive the standard 'find the CREATE_FAILED event for logical id X' remediation.
Fix: the stack provisioning engine's event recorder in the cloudformation service crate — record an event per logical resource for each status transition (CREATE_IN_PROGRESS, CREATE_FAILED/CREATE_COMPLETE, and the rollback DELETE_* transitions) with LogicalResourceId, PhysicalResourceId, ResourceType and the resource's own ResourceStatusReason, and keep the 'following resource(s) failed to create' aggregate on the stack-level event only.
Traces: cloudformation.create-stack-s3-bucket-name-conflict-rolls-back.

Cause 2 of 2: resource properties are never validated against the resource type's schema; an unknown property is forwarded verbatim to the target service.
AWS: CloudFormation validates each resource's property bag against the resource type schema asynchronously, after CreateStack has returned a StackId: the stack goes CREATE_FAILED -> ROLLBACK_COMPLETE and the CREATE_FAILED reason is CloudFormation's own generic text, 'Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.' (the check records this as verified against real AWS us-west-2; the error COUNT is part of the reason). The model corroborates that this is CloudFormation's own step: StackEvent.DetailedStatus documents VALIDATION_FAILED as 'template validation failed because of invalid properties in the template. The ResourceStatusReason field shows what properties are defined incorrectly.' A quoted integer for an Integer property is legal and must be coerced, not refused.
Emulator: an AWS::SQS::Queue carrying 'ThisPropertyDoesNotExist' is passed straight through to SQS, which refuses the attribute, and the stack's reason becomes 'The following resource(s) failed to create: [Q]. InvalidAttributeName: Unknown Attribute ThisPropertyDoesNotExist.' — no schema validation step, no 'Validation failed with N error(s)' reason, no VALIDATION_FAILED detailed status.
Why: a template whose properties are wrong fails for the wrong reason and with a service-specific code, so a client cannot tell a template defect from a data-plane refusal; and any resource type whose provider happens to tolerate extra properties would be accepted, deploying a resource real AWS refuses.
Fix: the resource-provisioning path of the cloudformation service crate — validate each resource's properties against the registered resource-type schema before invoking the provider, emit the per-resource CREATE_FAILED event with CloudFormation's generic validation reason and DetailedStatus VALIDATION_FAILED, and keep documented type coercion (quoted integers) accepted.
Traces: cloudformation.resource-schema-rejects-unknown-and-mistyped-properties.

## Evidence
Cause 1: AssertionError: expected a CREATE_FAILED event naming the validation failure, got ['The following resource(s) failed to create: [Bucket]. BucketAlreadyOwnedByYou: Your previous request to create the named bucket succeeded and you already own it.'] — the stack did reach ROLLBACK_COMPLETE, but the only CREATE_FAILED event is the stack-level aggregate and no event exists for LogicalResourceId 'Bucket'.
Cause 2: AssertionError: [unknown property] the CREATE_FAILED reason must report a resource-schema validation failure ('Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.'), got 'The following resource(s) failed to create: [Q]. InvalidAttributeName: Unknown Attribute ThisPropertyDoesNotExist.'

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: CreateBucket a bucket directly, then CreateStack a template whose AWS::S3::Bucket sets BucketName to that name; wait for ROLLBACK_COMPLETE and call DescribeStackEvents — observe no event with LogicalResourceId 'Bucket' and the provider error folded into the stack-level CREATE_FAILED reason.
3. Cause 2: CreateStack a template with one AWS::SQS::Queue carrying an unknown property (e.g. ThisPropertyDoesNotExist); the call returns a StackId, the stack reaches ROLLBACK_COMPLETE, and the CREATE_FAILED reason is SQS's InvalidAttributeName instead of CloudFormation's 'Validation failed with 1 error(s). ...' reason.
4. Re-replay the two check ids in trace_ids to confirm the fix.
