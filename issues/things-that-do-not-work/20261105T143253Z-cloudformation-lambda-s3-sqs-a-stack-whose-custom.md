Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__s3__sqs.cfn-custom-resource-lambda-invocation-positive
Issue-Key: cloudformation__lambda__s3__sqs.cfn-custom-resource-lambda-invocation-positive|_deploy_cfn#3|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# cloudformation__lambda__s3__sqs: a stack whose Custom:: resource is backed by a Lambda ServiceToken rolls back instead of completing on the provider's SUCCESS response

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause of service `cloudformation__lambda__s3__sqs`, grounded in the failing behavioral check in trace_ids. The replay report marked it neither ALREADY FILED nor OWNED. The same underlying provisioner gap is filed separately for the `cloudformation__lambda__sqs` service token (a bundle also covering the DescribeStacks Outputs defect) and the open issues 20261103T061702Z-cloudformation-lambda-ssm-3-net-new-root-causes.md and 20261104T232822Z-cloudformation-iam-lambda-s3-4-net-new-root.md name adjacent Lambda-backed custom-resource symptoms for other service tokens; they are named here so a fixer can close them together. This is not a refusal, so it does not reopen a resolved issue.

=== Cause 1 of 1: CloudFormation does not invoke a Lambda-backed Custom:: provider to completion, so the stack rolls back ===
AWS: the custom-resource protocol is not in the Smithy model (recorded behavior only; AWS reference: 'Custom resources' / crpg-ref in the CloudFormation User Guide). For a `Custom::*` resource whose `ServiceToken` is a Lambda function ARN, CloudFormation invokes the function with the provider request and holds the resource in CREATE_IN_PROGRESS until the function PUTs a response document to the request's presigned `ResponseURL`. `Status=SUCCESS` takes the resource to CREATE_COMPLETE and the stack to CREATE_COMPLETE, and the response's `Data` map becomes readable through `Fn::GetAtt`, so a stack Output declared as `{"Fn::GetAtt": ["<Logical>", "<Key>"]}` resolves to the value only the running function could have produced.
Emulator: the stack settles in `ROLLBACK_COMPLETE` instead: `AssertionError: Stack <stack name> ended in ROLLBACK_COMPLETE`. The per-run echo value the function would have returned is therefore never exposed in the stack's Outputs, and the resource is not provisioned.
Why: a client deploying any template with a Lambda-backed custom resource — the dominant form, and what every CDK/SAM custom-resource construct emits — gets a rolled-back stack rather than provisioned resources, and gets no provider-side reason for the rollback, so it cannot tell a real provider failure from CloudFormation never having invoked the provider at all.
Fix: the CloudFormation provisioner's `Custom::*` / `AWS::CloudFormation::CustomResource` arm must dispatch to Lambda `Invoke` when `ServiceToken` names a function, serve the presigned `ResponseURL` in process, hold the resource at a stabilisation barrier until the response lands, and map `SUCCESS` onto CREATE_COMPLETE carrying the response's `PhysicalResourceId` and `Data` into `Fn::GetAtt` and stack Outputs — rather than failing the resource and rolling the stack back.
Traces: cloudformation__lambda__s3__sqs.cfn-custom-resource-lambda-invocation-positive.

## Evidence
cloudformation__lambda__s3__sqs.cfn-custom-resource-lambda-invocation-positive [mismatch]:
  expected: after CreateStack (with CAPABILITY_NAMED_IAM) over a template whose Custom:: resource's ServiceToken is a Lambda function ARN, the function is invoked during CREATE, answers SUCCESS, the stack reaches StackStatus CREATE_COMPLETE, and the stack Output declared as Fn::GetAtt on the provider's Data carries the per-run EchoValue only the running function can return.
  actual:   AssertionError: Stack <stack name> ended in ROLLBACK_COMPLETE

(The stack name in that message is a per-run value; the requirement is the terminal StackStatus and the resolved Output.)

## Steps to Reproduce
Boot the emulator and replay cloudformation__lambda__s3__sqs.cfn-custom-resource-lambda-invocation-positive from suite simparity-behavioral-aws-v1.

By hand:
1. iam.CreateRole trusting lambda.amazonaws.com; lambda.CreateFunction (python3.12) whose handler reads event["ResponseURL"] and PUTs {"Status":"SUCCESS","PhysicalResourceId":"<pid>","StackId":event["StackId"],"RequestId":event["RequestId"],"LogicalResourceId":event["LogicalResourceId"],"Data":{"Echo":event["ResourceProperties"]["EchoValue"]}}.
2. cloudformation.CreateStack(Capabilities=["CAPABILITY_NAMED_IAM"]) with TemplateBody {"Resources":{"Thing":{"Type":"Custom::Echo","Properties":{"ServiceToken":"<function ARN>","EchoValue":"<unique value>"}}},"Outputs":{"Echo":{"Value":{"Fn::GetAtt":["Thing","Echo"]}}}}.
3. Poll cloudformation.DescribeStacks until the stack is terminal. AWS: CREATE_COMPLETE with Outputs Echo == the unique value. Emulator: ROLLBACK_COMPLETE.
