Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack cloudformation__dynamodb__lambda.cdk-custom-resource-provider-waits-for-iscomplete cloudformation__dynamodb__lambda.cdk-nested-stack-output-feeds-a-parent-function-that-writes-the-nested-table
Issue-Key: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack|run#13|AssertionError|assert rc != 0, (
Issue-Key: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-waits-for-iscomplete|_stack_outputs#14|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in described.get("Outputs", [])}
Issue-Key: cloudformation__dynamodb__lambda|DescribeStacks|ValidationError
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation/dynamodb/lambda: 3 net-new root causes — custom-resource Provider timeout ignored, stack Outputs carry no OutputValue, nested child stacks not describable

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Three distinct net-new root causes. Open issues 20261023T114649Z-cloudformation-outputs-carry-no-outputvalue-failed-resources-never.md and 20261025T015306Z-cloudformation-stack-events-and-resource-views-are-incomplete.md describe the same families for other replay keys but carry none of the keys below.

Cause 1 of 3: a CDK custom-resource Provider's total_timeout does not fail the create.
AWS: recorded behavior only — when the Provider's isComplete handler keeps returning IsComplete=false, the framework gives up at total_timeout and the custom resource fails, which fails the stack (CREATE_FAILED/ROLLBACK_COMPLETE) and `cdk deploy` exits non-zero.
Emulator: `cdk deploy` exits 0 — the custom resource reaches CREATE_COMPLETE (and the stack CREATE_COMPLETE) despite isComplete never returning true; the output value is 'undefined'.
Why: an asynchronous custom resource that never becomes ready is reported as ready, so a stack that must fail is deployed.
Fix: the waiter/timeout accounting for AWS::CloudFormation::CustomResource provisioning (isComplete polling with a deadline).
Traces: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack.

Cause 2 of 3: stack Outputs entries carry no OutputValue.
AWS: recorded behavior only — DescribeStacks returns each Output as {OutputKey, OutputValue[, ExportName, Description]}; the value is always present once the stack is CREATE_COMPLETE.
Emulator: the Outputs entry has no OutputValue key at all (KeyError: 'OutputValue').
Why: every consumer that reads a deployed value out of a stack output fails outright.
Fix: output resolution in the stack state / DescribeStacks serializer.
Traces: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-waits-for-iscomplete.

Cause 3 of 3: a nested child stack is not addressable as a stack.
AWS: recorded behavior only — a nested stack's physical id is the child stack's ARN, so DescribeStacks on it returns the child and its Outputs can feed the parent.
Emulator: DescribeStacks raises ValidationError 'Stack with id <parent>-DataNestedStackDataNestedStackResource<suffix> does not exist' — the child exists only as a generated name.
Why: nested-stack outputs cannot be read, so parent resources that consume them cannot be configured.
Fix: nested-stack provisioning must create a real child stack and return its ARN as the physical resource id.
Traces: cloudformation__dynamodb__lambda.cdk-nested-stack-output-feeds-a-parent-function-that-writes-the-nested-table.

## Evidence
C1: "`cdk deploy` exited 0 with isComplete permanently returning False -- the Provider's 40s total_timeout must fail the create, not let it settle"; the deploy log shows CREATE_COMPLETE for Provider/framework-onEvent, Waited/Default and the stack, with 'Value = undefined'.
C2: KeyError: 'OutputValue' when reading the stack's Outputs.
C3: botocore ClientError (ValidationError) on DescribeStacks: 'Stack with id <parent>-DataNestedStackDataNestedStackResource<suffix> does not exist'.
Stack names, suffixes and the 40s timeout value are per-run/per-app values; the behaviours are the requirements.

## Steps to Reproduce
C1: deploy a CDK app whose custom-resource Provider has an isComplete handler that always returns IsComplete=false and a total_timeout, and check the exit code and stack status. C2: read DescribeStacks Outputs for that app. C3: deploy a CDK app with a nested stack and call DescribeStacks on the nested stack resource's PhysicalResourceId.
