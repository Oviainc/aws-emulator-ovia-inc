Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.cfn-custom-resource-lambda-missing-s3-permission-negative cloudformation__iam__lambda__s3.cfn-nested-stack-template-wrong-region-negative cloudformation__iam__lambda__s3.cfn-passrole-denied-for-service-role-negative cloudformation__iam__lambda__s3.cli-cfn-deploy-stack-describe-resources-iam-lambda-positive
Issue-Key: cloudformation__iam__lambda__s3.cfn-custom-resource-lambda-missing-s3-permission-negative|run#11|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", f"expected rollback, got {bad_status}"
Issue-Key: cloudformation__iam__lambda__s3.cfn-nested-stack-template-wrong-region-negative|run#10|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", f"expected rollback, got {bad_status}"
Issue-Key: cloudformation__iam__lambda__s3.cfn-passrole-denied-for-service-role-negative|run#22|AssertionError|assert final_status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__iam__lambda__s3.cli-cfn-deploy-stack-describe-resources-iam-lambda-positive|_run_shell#25|AssertionError|raise AssertionError(
Issue-Commit: 6ff397d47b7f23b010f8590f84afedb6b986e178

# cloudformation__iam__lambda__s3: 4 net-new root causes — custom-resource failures, nested-stack TemplateURL validation and iam:PassRole are all no-ops, and Lambda Invoke is refused

> _Note: this issue was identified against the code base at commit `6ff397d47b7f23b010f8590f84afedb6b986e178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Four distinct net-new root causes in this service, all reached through CreateStack-driven flows. Each must be closed before this issue is submitted.

Cause 1 of 4: a Lambda-backed custom resource that fails is treated as a success, so the stack never rolls back.
AWS: recorded behavior only (CloudFormation custom resources are provider-driven, not Smithy-modelled). A stack containing a Custom::* resource whose Lambda handler cannot do its work (here the handler's execution role carries no s3:PutObject at all) sends a FAILED response to the pre-signed response URL; CloudFormation then marks that logical resource CREATE_FAILED with a ResourceStatusReason naming the handler's own error (the recording expects the substring "AccessDenied"), and the stack, under the default rollback behaviour, ends in ROLLBACK_COMPLETE with the side effect absent (HeadObject on the key the handler would have written answers 404).
Emulator: DescribeStacks reports StackStatus CREATE_COMPLETE for that same stack. The custom resource is recorded as created without the handler ever being invoked or its response consulted, so no CREATE_FAILED event and no AccessDenied reason exist and nothing rolls back.
Why: a client that uses a custom resource as a deploy-time gate (seeding a bucket, validating a configuration, registering an external record) gets a green stack whose work never happened. Negative deploy tests pass vacuously and the "deployed" account state silently diverges from the template.
Fix: the CloudFormation stack provisioner's Custom::*/AWS::CloudFormation::CustomResource handler — it must invoke the ServiceToken function, honour the SUCCESS/FAILED response, and feed FAILED into the stack's failure/rollback state machine.
Traces: cloudformation__iam__lambda__s3.cfn-custom-resource-lambda-missing-s3-permission-negative (AssertionError: expected rollback, got CREATE_COMPLETE).

Cause 2 of 4: AWS::CloudFormation::Stack accepts a TemplateURL that is an S3 ARN instead of an HTTPS URL.
AWS: recorded behavior only. TemplateURL must be an HTTPS S3 URL. Given `arn:aws:s3:::<bucket>/<key>` the nested-stack resource goes CREATE_FAILED with exactly one ResourceStatusReason whose fixed text is "The location for an Amazon S3 bucket must start with https://" (no variable parts), the parent stack ends ROLLBACK_COMPLETE, and ListStackResources reports the NestedStack resource as DELETE_COMPLETE. The identical child by its `https://s3.<region>.amazonaws.com/<bucket>/<key>` URL reaches CREATE_COMPLETE.
Emulator: the parent stack reaches CREATE_COMPLETE with the S3-ARN TemplateURL; the refusal is never raised, so neither the CREATE_FAILED reason nor the rollback nor the DELETE_COMPLETE nested-resource row exists.
Why: a template with a malformed nested TemplateURL deploys "successfully" in the emulator and fails in AWS, which is the exact inversion of the signal a deploy pipeline relies on.
Fix: the validation in the CloudFormation AWS::CloudFormation::Stack resource handler that resolves TemplateURL — reject any location that does not start with https:// and surface that message as the resource's failure reason.
Traces: cloudformation__iam__lambda__s3.cfn-nested-stack-template-wrong-region-negative (AssertionError: expected rollback, got CREATE_COMPLETE).

Cause 3 of 4: CreateStack does not evaluate iam:PassRole for the deploying principal.
AWS: recorded behavior only. When a limited principal creates a stack whose AWS::Lambda::Function takes `Role: !GetAtt ExecRole.Arn` and that principal has no iam:PassRole for the role, the Function resource goes CREATE_FAILED with a ResourceStatusReason containing "PassRole", the stack ends ROLLBACK_COMPLETE (the only other resource, the role, is deletable), and the function is absent — Lambda GetFunction answers 404 ResourceNotFoundException with message "Function not found: arn:aws:lambda:<region>:<account>:function:<function-name>" (fixed text "Function not found: " plus the request-derived function ARN).
Emulator: the stack created by the limited principal reaches CREATE_COMPLETE; the PassRole denial is never evaluated, so no CREATE_FAILED event names PassRole and the function is created.
Why: the emulator grants any caller the ability to pass any role through CloudFormation. Least-privilege deploy roles cannot be tested, and a stack that AWS would refuse deploys cleanly.
Fix: the authorization step of the CloudFormation stack provisioner for resources that carry a role property (iam:PassRole on the calling principal for the referenced role ARN), reported as the resource's CREATE_FAILED reason.
Traces: cloudformation__iam__lambda__s3.cfn-passrole-denied-for-service-role-negative (AssertionError: expected stack … to roll back after the denied PassRole, got 'CREATE_COMPLETE').

Cause 4 of 4: Lambda Invoke refuses a real invocation with 501 NotImplemented.
AWS: Smithy: lambda#Invoke returns InvocationResponse (StatusCode, Payload) and its modelled errors are client/service faults such as ResourceNotFoundException, InvalidRequestContentException and Unhandled/FunctionError — NotImplemented is not among them; a deployed python3.12 function invoked at $LATEST answers 200 with the handler's payload.
Emulator: `aws lambda invoke --function-name <fn>` exits 254 with "An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:<region>:<account>:function:<fn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator…".
Why: every end-to-end flow that proves a CFN-deployed function actually runs is unreachable, so deploy checks can only assert control-plane shape.
The open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md describes the same refusal but does not carry this check's replay key, so this check is reported here and should be closed together with it.
Fix: the Lambda Invoke handler's execution path (the in-process runtime for a deployment package).
Traces: cloudformation__iam__lambda__s3.cli-cfn-deploy-stack-describe-resources-iam-lambda-positive.

## Evidence
Expected vs actual, per cause:
1) cfn-custom-resource-lambda-missing-s3-permission-negative — expected DescribeStacks StackStatus=ROLLBACK_COMPLETE, a Writer CREATE_FAILED ResourceStatusReason containing "AccessDenied", and HeadObject 404 on the key; actual StackStatus=CREATE_COMPLETE (AssertionError: expected rollback, got CREATE_COMPLETE).
2) cfn-nested-stack-template-wrong-region-negative — expected ROLLBACK_COMPLETE plus exactly one NestedStack CREATE_FAILED reason "The location for an Amazon S3 bucket must start with https://" and the NestedStack resource DELETE_COMPLETE; actual CREATE_COMPLETE (AssertionError: expected rollback, got CREATE_COMPLETE).
3) cfn-passrole-denied-for-service-role-negative — expected ROLLBACK_COMPLETE with a Function CREATE_FAILED reason containing "PassRole" and GetFunction 404 ResourceNotFoundException "Function not found: arn:aws:lambda:<region>:<account>:function:<fn>"; actual 'CREATE_COMPLETE'.
4) cli-cfn-deploy-stack-describe-resources-iam-lambda-positive — expected HTTP 200 InvocationResponse from Invoke; actual 501-class ClientError code NotImplemented, CLI rc=254, message "Invoke resolved <function-arn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce. InvocationType=DryRun validates an invocation without running the function."

## Steps to Reproduce
1) Replay cloudformation__iam__lambda__s3.cfn-custom-resource-lambda-missing-s3-permission-negative: create a bucket, create a Lambda role with logs permissions only, create a function, CreateStack with a Custom::Resource whose ServiceToken is that function and which must PutObject into the bucket; poll DescribeStacks to a terminal status.
2) Replay cloudformation__iam__lambda__s3.cfn-nested-stack-template-wrong-region-negative: put a child template object in a bucket, CreateStack with an AWS::CloudFormation::Stack whose TemplateURL is `arn:aws:s3:::<bucket>/<key>`; poll DescribeStacks and read DescribeStackEvents/ListStackResources for the NestedStack logical id.
3) Replay cloudformation__iam__lambda__s3.cfn-passrole-denied-for-service-role-negative: create an IAM user/credentials without iam:PassRole, CreateStack (Capabilities=[CAPABILITY_IAM]) as that principal with an AWS::IAM::Role plus an AWS::Lambda::Function referencing it; poll DescribeStacks, then GetFunction.
4) Replay cloudformation__iam__lambda__s3.cli-cfn-deploy-stack-describe-resources-iam-lambda-positive: deploy the stack, then `aws lambda invoke --function-name <fn> --payload file://… out.json`.
