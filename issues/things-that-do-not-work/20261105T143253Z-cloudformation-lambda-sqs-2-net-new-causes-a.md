Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative cloudformation__lambda__sqs.deploy-cloudformation-2-lambdahook-rejects-deny-queue
Issue-Key: cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals|_deploy_cfn#9|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative|_wait_for_record#11|Timeout|resp = sqs.receive_message(
Issue-Key: cloudformation__lambda__sqs.deploy-cloudformation-2-lambdahook-rejects-deny-queue|_outputs#15|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# cloudformation__lambda__sqs: 2 net-new causes — a Lambda-backed Custom:: resource rolls its stack back instead of completing on the provider's SUCCESS, and DescribeStacks Outputs entries omit OutputValue

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes of service `cloudformation__lambda__sqs`, grounded in the three failing behavioral checks in trace_ids. None of them was marked ALREADY FILED or OWNED by the replay report. The open issue 20261105T022225Z-cloudformation-lambda-sqs-2-net-new-causes-the.md names adjacent custom-resource symptoms for this same service token (PhysicalResourceId not adopted, FAILED response handling) and the open issue 20261102T101202Z-cloudformation-dynamodb-lambda-3-net-new-root-causes.md and 20261105T022225Z-apigateway-cloudformation-lambda-3-net-new-causes-describestacks.md both name 'stack Outputs carry no OutputValue' for other service tokens; they are named here so a fixer can close them together. Neither cause below is a refusal, so neither reopens a resolved issue.

=== Cause 1 of 2: a Custom:: resource whose ServiceToken is a Lambda function ARN never completes — the stack ends in ROLLBACK_COMPLETE ===
AWS: the custom-resource protocol is not in the Smithy model (recorded behavior only; AWS reference: crpg-ref / 'Custom resources' in the CloudFormation User Guide). When a `Custom::*` resource's `ServiceToken` is a Lambda function ARN, CloudFormation INVOKES that function asynchronously with the provider request (RequestType, RequestId, StackId, LogicalResourceId, ResponseURL, ResourceProperties, plus PhysicalResourceId/OldResourceProperties on Update and Delete) and holds the resource in CREATE_IN_PROGRESS until the function PUTs a response to the presigned `ResponseURL`. A response with `Status=SUCCESS` takes the resource to CREATE_COMPLETE, the stack to CREATE_COMPLETE, and makes the response's `Data` readable through `Fn::GetAtt`; stack DELETE then sends the function a `Delete` request for the same PhysicalResourceId. A SUCCESS response must never roll the stack back.
Emulator: the stack settles in `ROLLBACK_COMPLETE` on CREATE: `AssertionError: Stack <stack name> ended in ROLLBACK_COMPLETE` where the control expectation is CREATE_COMPLETE. The second check, which exercises the FAILED leg and then the SUCCESS leg of the same template, makes no progress at all and is killed at the harness ceiling (`Check timed out after 60.0s`). So neither the Create/Delete lifecycle pair nor the `Fn::GetAtt` round trip over a Lambda-backed provider is reachable.
Why: every template whose provider is a Lambda function — which is the dominant form, and the form every CDK/SAM custom-resource construct and every `AWS::CloudFormation::CustomResource` with a Lambda ServiceToken emits — fails to deploy. The client gets a rolled-back stack with no provisioned resource and no provider-side diagnosis, rather than a completed stack; and because the rollback is reported as the stack's own outcome, the client cannot tell the provider was never satisfied.
Fix: the CloudFormation provisioner's `Custom::*` / `AWS::CloudFormation::CustomResource` arm must dispatch to Lambda `Invoke` (event invocation) when `ServiceToken` is a function ARN, serve the presigned `ResponseURL` in process, hold the resource at a stabilisation barrier until the response lands, and map `Status=SUCCESS` onto CREATE_COMPLETE (carrying `PhysicalResourceId` and `Data` into GetAtt/Outputs) instead of failing the resource and rolling the stack back. The Delete request on stack deletion must be driven from the same path.
Traces: cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals, cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative.

=== Cause 2 of 2: DescribeStacks returns Outputs entries with no OutputValue member ===
AWS: Smithy `com.amazonaws.cloudformation#Output` has members `OutputKey`, `OutputValue`, `Description`, `ExportName`; `DescribeStacks` returns `Stacks[].Outputs` as a list of these. For every output a template declares and whose `Value` resolves, AWS populates BOTH `OutputKey` and `OutputValue` — `OutputValue` carries the resolved string (here the `Ref`/`Fn::GetAtt` the template names, e.g. a queue URL or a function alias ARN). An Outputs entry is never emitted with the key alone.
Emulator: `DescribeStacks` answers with Outputs entries that have no `OutputValue` member, so a client indexing the documented member raises: `KeyError: 'OutputValue'` from `{o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}`. The outputs of a stack that deployed are therefore unreadable.
Why: reading a deployed stack's outputs is the normal hand-off between stacks and between a deploy and the code that consumes it. Every client that maps OutputKey->OutputValue (the AWS CLI's `--query 'Stacks[0].Outputs'`, CDK/SAM cross-stack wiring, and any SDK caller) crashes or sees an empty value instead of the resolved output, so nothing downstream of a deploy can be wired up.
Fix: the `DescribeStacks` serializer/state for stack outputs must emit `OutputValue` (and `Description`/`ExportName` when declared) alongside `OutputKey`, populated from the resolved template output value recorded at provisioning time.
Traces: cloudformation__lambda__sqs.deploy-cloudformation-2-lambdahook-rejects-deny-queue.

## Evidence
Cause 1 — cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals [mismatch]:
  expected: the Lambda-backed Custom:: resource receives a Create provider request, answers SUCCESS, the stack reaches CREATE_COMPLETE, Fn::GetAtt on the response Data resolves, and stack DELETE sends a Delete request for the same PhysicalResourceId
  actual:   AssertionError: Stack <stack name> ended in ROLLBACK_COMPLETE

  cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative [mismatch]:
  expected: the FAILED leg reaches ROLLBACK_COMPLETE with a CREATE_FAILED event carrying the provider's Reason and the pre-existing queue actually deleted from SQS; the SUCCESS leg of the same template deploys and echoes its value
  actual:   Check timed out after 60.0s — no progress past the provider handshake

Cause 2 — cloudformation__lambda__sqs.deploy-cloudformation-2-lambdahook-rejects-deny-queue [error]:
  expected: DescribeStacks -> Stacks[0].Outputs == [{"OutputKey": "<declared key>", "OutputValue": "<resolved value>"}, ...] per Smithy com.amazonaws.cloudformation#Output (OutputKey, OutputValue, Description, ExportName)
  actual:   KeyError: 'OutputValue' raised at `return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}` — the Outputs entries carry OutputKey but no OutputValue member

(The stack names and ids in the messages above are per-run values; the requirements are the StackStatus, the presence of the provider Create/Delete requests, and the presence of the OutputValue member.)

## Steps to Reproduce
Boot the emulator and replay the three checks in trace_ids from suite simparity-behavioral-aws-v1.

Cause 1, by hand:
1. iam.CreateRole trusting lambda.amazonaws.com; lambda.CreateFunction (python3.12) whose handler PUTs {"Status":"SUCCESS","PhysicalResourceId":"<pid>","StackId":...,"RequestId":...,"LogicalResourceId":...,"Data":{"Echo":"<value>"}} to event["ResponseURL"].
2. cloudformation.CreateStack with TemplateBody {"Resources":{"Q":{"Type":"AWS::SQS::Queue"},"Thing":{"Type":"Custom::Echo","Properties":{"ServiceToken":"<function ARN>","EchoValue":"<value>"}}},"Outputs":{"Echo":{"Value":{"Fn::GetAtt":["Thing","Echo"]}}}} and poll DescribeStacks.
3. AWS: the function is invoked and the stack reaches CREATE_COMPLETE. Emulator: the stack reaches ROLLBACK_COMPLETE.
4. DeleteStack: AWS sends the function a Delete request for the same PhysicalResourceId.

Cause 2, by hand:
1. cloudformation.CreateStack with any template declaring an Outputs section, e.g. {"Resources":{"Q":{"Type":"AWS::SQS::Queue"}},"Outputs":{"QueueUrl":{"Value":{"Ref":"Q"}}}}; wait for CREATE_COMPLETE.
2. cloudformation.DescribeStacks(StackName=<stack id>) and read Stacks[0].Outputs — each entry must carry both OutputKey and OutputValue; the emulator returns entries without OutputValue, so `o["OutputValue"]` raises KeyError.
