Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift|run#40|AssertionError|assert [d["StackResourceDriftStatus"] for d in table_drifts] == ["MODIFIED"], (
Issue-Commit: b0d768b589014546a522750598bcb5739ddb6c75

# cloudformation: drift detection reports an existing, out-of-band-modified AWS::DynamoDB::Table as DELETED instead of MODIFIED

> _Note: this issue was identified against the code base at commit `b0d768b589014546a522750598bcb5739ddb6c75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackResourceDrifts

## Description
Cause 1 of 1: resource-level drift classification never reports MODIFIED — a resource that still exists but whose actual properties differ from the template is classified DELETED, and so carries no PropertyDifferences/ActualProperties. Failing check: cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift.

AWS: Smithy (com.amazonaws.cloudformation) — StackResourceDrift.StackResourceDriftStatus is an enum of IN_SYNC | MODIFIED | DELETED | NOT_CHECKED | UNKNOWN | UNSUPPORTED, documented as: 'DELETED: The resource differs from its expected template configuration because the resource has been deleted' and 'MODIFIED: One or more resource properties differ from their expected values (as defined in the stack template and any values specified as template parameters)'. The same shape documents that ExpectedProperties/ActualProperties 'will not be present' for a DELETED resource, and PropertyDifferences is 'present only for resources whose StackResourceDriftStatus is MODIFIED'. So after a stack-managed DynamoDB table has its PointInTimeRecoverySpecification flipped out of band (UpdateContinuousBackups, with the template still saying disabled), DetectStackDrift must settle the stack as DRIFTED and DescribeStackResourceDrifts must return that one AWS::DynamoDB::Table entry with StackResourceDriftStatus=MODIFIED plus the PropertyDifferences for the changed property — never DELETED, because the physical resource is still there and still readable.

Emulator: the stack-level verdict is reached, but the per-resource entry for the AWS::DynamoDB::Table comes back as DELETED. The replayed assertion is `the table's drift status was ['DELETED'], expected exactly ['MODIFIED']`. The same check continues to read and write the table through DynamoDB before and after the drift call, so the resource demonstrably exists; the emulator has simply mapped 'actual properties do not match expected' onto DELETED instead of comparing properties and emitting MODIFIED.

Why: a client that relies on drift to tell 'someone changed a property' from 'someone deleted the resource' gets the opposite answer. DELETED tells remediation tooling (and `cdk drift`, CloudFormation consoles, drift-gated pipelines) to re-create or import the resource, when the correct action is an in-place update of the drifted property; and because DELETED suppresses PropertyDifferences/ExpectedProperties/ActualProperties, the caller never learns which property drifted, so no automated remediation is possible at all.

Fix: the drift-detection comparison in CloudFormation's drift handlers/state — the step that resolves each managed resource's actual properties from the owning service and classifies the result. It must distinguish 'physical resource absent' (DELETED) from 'present with differing properties' (MODIFIED), and for MODIFIED populate ExpectedProperties, ActualProperties and PropertyDifferences for the differing properties; reverting the out-of-band change must then return the resource to IN_SYNC.

Traces: cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift (arm 4 — out-of-band PITR enable on the stack's table, then DetectStackDrift + DescribeStackResourceDrifts).

## Evidence
Failing check cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift [mismatch]:

  File "check_cloudformation_cdk_diff_sees_template_change_not_out_of_band_drift.py", line 158, in run
    assert [d["StackResourceDriftStatus"] for d in table_drifts] == ["MODIFIED"]
  AssertionError: the table's drift status was ['DELETED'], expected exactly ['MODIFIED']

Expected (AWS): DescribeStackResourceDrifts returns exactly one entry with ResourceType=AWS::DynamoDB::Table and StackResourceDriftStatus=MODIFIED, with PropertyDifferences naming the drifted property (PointInTimeRecoverySpecification) and both ExpectedProperties and ActualProperties present.
Actual (emulator): that entry's StackResourceDriftStatus is DELETED (which by the model also means no PropertyDifferences/ExpectedProperties/ActualProperties), even though the table is alive — the check reads PITR status from DynamoDB immediately before the drift call and GetItem/PutItem on the same table immediately after.

## Steps to Reproduce
1. Deploy a stack containing an AWS::DynamoDB::Table whose PointInTimeRecoverySpecification.PointInTimeRecoveryEnabled is false (the check uses `cdk deploy` of the cdk_table_diffable app; a plain CreateStack with the same resource is equivalent).
2. Confirm DynamoDB DescribeContinuousBackups reports PITR DISABLED for the created table.
3. Change the resource out of band: dynamodb UpdateContinuousBackups with PointInTimeRecoveryEnabled=true, and poll until PITR reports ENABLED. The stack template still says disabled.
4. Call cloudformation DetectStackDrift on the stack and wait for the detection to complete, then call DescribeStackResourceDrifts for that detection id.
5. Observe the AWS::DynamoDB::Table entry: the emulator reports StackResourceDriftStatus=DELETED; AWS reports MODIFIED with PropertyDifferences for PointInTimeRecoverySpecification.
6. Revert the out-of-band change (PointInTimeRecoveryEnabled=false) and re-detect: the resource must come back IN_SYNC.
