Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-workflow
Issue-Key: cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-workflow|hard#10|AssertionError|raise AssertionError(self._summary(name, _one_line(exc))) from exc
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# CFN-deployed search-indexing pipeline fails immediately: stack Outputs have no OutputValue

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One root cause.

Cause 1 of 1: DescribeStacks returns Outputs entries without OutputValue, so the multi-service pipeline workflow cannot obtain the deployed domain's identifiers.
AWS: a stack that has reached CREATE_COMPLETE returns, for each template Output, an Output member carrying both OutputKey and the resolved OutputValue. Smithy: cloudformation#Output = {OutputKey, OutputValue, Description, ExportName}; OutputValue is the resolved value of the template's `Value` expression.
Emulator: the Outputs entries omit OutputValue; the workflow's first leg ('domain') fails with 'OutputValue' and the harness reports LEGS 0/11 PASS. Nothing after the deploy leg (S3 notification, Lambda indexing, CloudWatch Logs, OpenSearch writes) is exercised.
Why: every downstream leg of a deployment-ordered workflow addresses resources by the identifiers the stack exports. With no OutputValue the client cannot discover them, so an entire deployed estate is undiscoverable even though the stack claims success.
Fix: CloudFormation output resolution/serialization — resolve template Output `Value` intrinsics at stack completion and emit OutputValue in the DescribeStacks Output serializer. (Same component as the cloudformation__iam__opensearch__sts finding filed in this sweep; filed separately because it belongs to a different service grouping.)
Traces: cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-workflow.

## Evidence
Expected: Stacks[0].Outputs entries contain both OutputKey and OutputValue after CREATE_COMPLETE.
Actual: AssertionError: LEGS 0/11 PASS | domain=FAIL / WORKFLOW FAILED at domain: 'OutputValue' (KeyError on the missing member).

## Steps to Reproduce
1. Boot the emulator.
2. Deploy the check's search-indexing pipeline template via cfn.create_stack and wait for completion.
3. Read Stacks[0]['Outputs'] — OutputValue is absent from each entry.
4. Replay cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-workflow.
