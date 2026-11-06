Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.deploy-ddb-stream-lambda-cloudwatch-dashboard cloudwatch__dynamodb__lambda.deploy-ddb-table-stream-esm-cfn-lambda-trigger cloudwatch__dynamodb__lambda.deploy-lambda-execution-role-missing-cloudwatch-permission-negative
Issue-Key: cloudwatch__dynamodb__lambda.deploy-ddb-stream-lambda-cloudwatch-dashboard|_deploy_cfn#5|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: cloudwatch__dynamodb__lambda.deploy-ddb-table-stream-esm-cfn-lambda-trigger|_deploy_cfn#5|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: cloudwatch__dynamodb__lambda.deploy-lambda-execution-role-missing-cloudwatch-permission-negative|_deploy_cfn#8|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: cc6137e930728edc173d6e704b5a7c1716e7fffd

# cloudwatch__dynamodb__lambda: a CFN stack wiring a DynamoDB stream to a Lambda via AWS::Lambda::EventSourceMapping now ends ROLLBACK_COMPLETE instead of CREATE_COMPLETE (1 cause)

> _Note: this issue was identified against the code base at commit `cc6137e930728edc173d6e704b5a7c1716e7fffd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation stack whose template declares an AWS::DynamoDB::Table with a StreamSpecification, an AWS::IAM::Role, an AWS::Lambda::Function (python3.12, inline Code.ZipFile) and an AWS::Lambda::EventSourceMapping from the table's StreamArn to that function reaches the terminal status ROLLBACK_COMPLETE instead of CREATE_COMPLETE. Failing checks: cloudwatch__dynamodb__lambda.deploy-ddb-stream-lambda-cloudwatch-dashboard, cloudwatch__dynamodb__lambda.deploy-ddb-table-stream-esm-cfn-lambda-trigger, cloudwatch__dynamodb__lambda.deploy-lambda-execution-role-missing-cloudwatch-permission-negative. The three templates share exactly that resource set (one adds an AWS::CloudWatch::Dashboard, two declare a second AWS::DynamoDB::Table for results); the common denominator is the stream-table + role + function + EventSourceMapping wiring.

AWS: Smithy cloudformation CreateStack accepts the template and DescribeStacks reports StackStatus from the StackStatus enum (CREATE_IN_PROGRESS -> CREATE_COMPLETE on success; ROLLBACK_IN_PROGRESS -> ROLLBACK_COMPLETE only after a resource create FAILED and the stack was rolled back). Every resource type in these templates is a supported AWS type, the ESM's EventSourceArn is resolved with Fn::GetAtt [Table, StreamArn] on a table the same stack creates with StreamSpecification, FunctionName with Ref, and the stack is submitted with Capabilities CAPABILITY_IAM/CAPABILITY_NAMED_IAM, so on AWS the stack settles CREATE_COMPLETE and each declared resource exists in its owning service afterwards. Recorded behavior: the three checks' deploy phase succeeded against real AWS and went on to inspect the created mapping.
Emulator: the stack settles in ROLLBACK_COMPLETE, so the deploy helper raises `AssertionError: Stack <name> ended in ROLLBACK_COMPLETE` and the whole scenario stops at its first step. No error code or message is surfaced to the caller by CreateStack itself — the refusal is only visible as the terminal stack status, and the replay output carries no resource-level failure reason.
Why: a client that provisions a DynamoDB-stream-to-Lambda pipeline declaratively (the standard serverless pattern) gets no stack at all: the table, role, function, mapping and dashboard are rolled back and deleted, DescribeStacks reports a failed stack, and every downstream read (ListEventSourceMappings, Invoke, GetMetricStatistics on the dashboard's series) has nothing to address. Nothing in the template can be adjusted to work around it.
Hypothesis: the resource provider for AWS::Lambda::EventSourceMapping (or the Fn::GetAtt StreamArn attribute it depends on) now fails during create and triggers the rollback — this is the same provider that open issue 20261105T022225Z-cloudwatch-dynamodb-lambda-2-net-new-causes-a.md reports as reaching CREATE_COMPLETE while leaving ListEventSourceMappings empty. These same three check ids appear in that issue's trace list, but the failure has moved earlier: the stack no longer reaches CREATE_COMPLETE at all, so the replay report does not mark these checks OWNED and the empty-listing defect described there is no longer the observable one.
Fix: the CloudFormation create path for this stack — the resource provider(s) for AWS::Lambda::EventSourceMapping / AWS::Lambda::Function / AWS::DynamoDB::Table stream attributes must create their resources successfully so the stack reaches CREATE_COMPLETE; and, whichever resource fails, its failure reason must be recorded as a stack event (ResourceStatus CREATE_FAILED with ResourceStatusReason) rather than only as a bare terminal ROLLBACK_COMPLETE, so the cause is diagnosable.
Traces: the three failing check ids listed above.

## Evidence
Expected: DescribeStacks on the created stack reports StackStatus == "CREATE_COMPLETE" for a template declaring AWS::DynamoDB::Table (with StreamSpecification), AWS::IAM::Role, AWS::Lambda::Function and AWS::Lambda::EventSourceMapping (plus, in one check, AWS::CloudWatch::Dashboard).

Actual (replay): the stack settles with StackStatus == "ROLLBACK_COMPLETE", producing in all three checks `AssertionError: Stack <stack-name> ended in ROLLBACK_COMPLETE` raised from the checks' _deploy_cfn helper after create_stack. (The stack name is a per-run generated value — e.g. sim2real-xsvc-stk-a8b13aa496ba, sim2real-xsvc-stk-3d3e5304f941, sim2real-xsvc-stk-6059bb8bd794; the requirement is the terminal status, not the name.) No resource-level CREATE_FAILED reason is visible in the replay output.

## Steps to Reproduce
1. CreateStack with Capabilities [CAPABILITY_IAM, CAPABILITY_NAMED_IAM] and a template declaring:
   - AWS::DynamoDB::Table with StreamSpecification (NEW_AND_OLD_IMAGES),
   - AWS::IAM::Role assumable by lambda.amazonaws.com with the stream-read and logs policies,
   - AWS::Lambda::Function (Runtime python3.12, Handler index.handler, inline Code.ZipFile, Role = Fn::GetAtt [Role, Arn]),
   - AWS::Lambda::EventSourceMapping with EventSourceArn = Fn::GetAtt [Table, StreamArn], FunctionName = Ref Function, StartingPosition TRIM_HORIZON, BatchSize 1,
   - (optionally) AWS::CloudWatch::Dashboard with a metric widget over AWS/Lambda IteratorAge.
2. Poll DescribeStacks until the stack reaches a terminal status.
3. Observe StackStatus == ROLLBACK_COMPLETE (AWS: CREATE_COMPLETE).
4. DescribeStackEvents on the stack — no CREATE_FAILED resource event with a ResourceStatusReason naming the resource that failed is available.
