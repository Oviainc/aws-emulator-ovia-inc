Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__kms__redshift.cfn-redshift-snapshotcopygrant-round-trip-and-delete cloudformation__kms__redshift.cfn-redshift-snapshotcopygrant-invalid-kms-key-rejected
Issue-Key: cloudformation__kms__redshift|CreateStack|ValidationError
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# cloudformation__kms__redshift: CreateStack refuses AWS::Redshift::SnapshotCopyGrant as an unrecognized resource type, so no KMS-backed snapshot-copy grant can be deployed from a template

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1 — AWS::Redshift::SnapshotCopyGrant is not a registered CloudFormation resource type.
AWS: recorded behavior only (CloudFormation's resource-type registry is not part of the Smithy model): CreateStack accepts a template declaring AWS::Redshift::SnapshotCopyGrant, provisions the grant through Redshift's CreateSnapshotCopyGrant (Ref/GetAtt resolve to the grant's SnapshotCopyGrantName), and a template naming a KmsKeyId that does not exist is rejected at the resource handler, leaving the stack in ROLLBACK_COMPLETE with a resource-level failure reason — the stack call itself succeeds (200, a StackId).
Emulator: CreateStack refuses the template outright with HTTP 400, error code ValidationError and message 'Template format error: Unrecognized resource types: [AWS::Redshift::SnapshotCopyGrant]'. The fixed text is 'Template format error: Unrecognized resource types: [' + the comma-separated list of the template's unregistered types + ']'; here that list is the single Redshift type. AWS only emits that message for types it genuinely does not know.
Why: no stack can create a Redshift snapshot-copy grant, so neither the deploy round-trip nor the KMS-key rejection path is reachable; a client whose template is valid on AWS gets a template-format error and cannot deploy at all.
Fix: the CloudFormation resource-type registry and the AWS::Redshift::SnapshotCopyGrant resource handler (create/read/delete mapped onto Redshift CreateSnapshotCopyGrant / DescribeSnapshotCopyGrants / DeleteSnapshotCopyGrant), including propagation of the Redshift refusal into the resource's failure reason.
Traces: cloudformation__kms__redshift.cfn-redshift-snapshotcopygrant-round-trip-and-delete, cloudformation__kms__redshift.cfn-redshift-snapshotcopygrant-invalid-kms-key-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 (cloudformation__kms__redshift: 0/2 passed).

Both checks fail on the first call: 'botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Redshift::SnapshotCopyGrant]'.

Expected: CreateStack returns a StackId; the round-trip check then waits for CREATE_COMPLETE and reads the grant back through Redshift, and the negative check waits for ROLLBACK_COMPLETE and reads the resource's failure reason. Actual: no stack is ever created.

## Steps to Reproduce
1. CreateStack with a template whose Resources contain one AWS::Redshift::SnapshotCopyGrant (SnapshotCopyGrantName, KmsKeyId) and an output Ref-ing it.
2. Observe 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::Redshift::SnapshotCopyGrant]' instead of a StackId.
3. Re-replay the checks listed in trace_ids.
