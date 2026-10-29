Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.cfn-lambda-role-getatt-arn-resolution-positive cloudformation__iam__lambda__s3.cli-cfn-deploy-stack-describe-resources-iam-lambda-positive cloudformation__iam__lambda__s3.cfn-iam-role-permissions-boundary-in-template-positive cloudformation__iam__lambda__s3.cfn-service-role-lacks-s3-getobject-for-template-negative cloudformation__iam__lambda__s3.cli-iam-role-create-attach-lambda-execution-positive cloudformation__iam__lambda__s3.cli-iam-role-deleted-breaks-lambda-invoke-negative cloudformation__iam__lambda__s3.cfn-delete-stack-removes-iam-role-and-lambda-positive cloudformation__iam__lambda__s3.s3-notification-triggers-lambda-role-tags-object-positive cloudformation__iam__lambda__s3.s3-notification-lambda-role-missing-put-tagging-negative cloudformation__iam__lambda__s3.s3-notification-cross-account-bucket-lambda-role-negative cloudformation__iam__lambda__s3.cfn-custom-resource-role-least-privilege-positive
Issue-Key: cloudformation__iam__lambda__s3|GetFunction|ResourceNotFoundException
Issue-Key: cloudformation__iam__lambda__s3.cli-cfn-deploy-stack-describe-resources-iam-lambda-positive|_run_shell#20|AssertionError|raise AssertionError(
Issue-Key: cloudformation__iam__lambda__s3.cfn-iam-role-permissions-boundary-in-template-positive|run#7|KeyError|assert role["PermissionsBoundary"]["PermissionsBoundaryArn"] == _BOUNDARY_ARN, (
Issue-Key: cloudformation__iam__lambda__s3.cfn-service-role-lacks-s3-getobject-for-template-negative|_error_code#9|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudformation__iam__lambda__s3.cli-iam-role-create-attach-lambda-execution-positive|_run_shell#22|AssertionError|raise AssertionError(
Issue-Key: cloudformation__iam__lambda__s3.cli-iam-role-deleted-breaks-lambda-invoke-negative|_invoke#19|AssertionError|assert proc.returncode == 0, f"expected successful invoke, got: {proc.stderr[-500:]}"
Issue-Key: cloudformation__iam__lambda__s3.cfn-delete-stack-removes-iam-role-and-lambda-positive|run#26|AssertionError|assert invoke_missing_message == invoke_not_found, (
Issue-Key: cloudformation__iam__lambda__s3.s3-notification-triggers-lambda-role-tags-object-positive|_poll_until#15|AssertionError|raise AssertionError(
Issue-Key: cloudformation__iam__lambda__s3.s3-notification-lambda-role-missing-put-tagging-negative|_poll_until#15|AssertionError|raise AssertionError(
Issue-Key: cloudformation__iam__lambda__s3.s3-notification-cross-account-bucket-lambda-role-negative|_error_code#15|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Key: cloudformation__iam__lambda__s3|GetObject|NoSuchKey
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# cloudformation__iam__lambda__s3: 8 net-new root causes — CFN-declared Lambda functions never exist, PermissionsBoundary dropped, TemplateURL fetch unauthorized, Invoke 501, S3 notifications inert

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Eight distinct net-new root causes surfaced by the cloudformation__iam__lambda__s3 behavioral group. Each must be closed before this issue is submitted. (An open issue 20261029T072657Z-cloudformation-iam-lambda-logs-a-stack-declaring-aws-e5cb3e.md and 20261029T182816Z-cloudformation-iam-lambda-logs-an-aws-lambda-function.md describe a related symptom in the cloudformation__iam__lambda__logs group, and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md describes a related Invoke symptom; neither carries these replay keys, so they do not cover these checks.)

Cause 1 of 8: an AWS::Lambda::Function declared by a stack that reports CREATE_COMPLETE does not exist in Lambda.
AWS: CloudFormation creates the modelled resource before the stack reaches CREATE_COMPLETE, so lambda:GetFunction / GetFunctionConfiguration on the function named by the stack's PhysicalResourceId (and by an Fn::GetAtt Arn output) answers 200 with that function's configuration. Smithy (lambda): GetFunction models ResourceNotFoundException (404) only for a function that does not exist.
Emulator: GetFunction and GetFunctionConfiguration answer 404 ResourceNotFoundException with message 'Function not found: arn:aws:lambda:<region>:<account>:function:<name>' for the exact function the stack declared and reported created. Fixed text 'Function not found: '; the ARN is the request's resolved function ARN.
Why: every consumer of a CFN-provisioned function (Fn::GetAtt resolution, `aws lambda get-function-configuration`, any downstream wiring) sees the function as nonexistent, so no stack that declares a function is usable.
Fix: the CloudFormation stack-provisioning path for the AWS::Lambda::Function resource type — it must create the resource in Lambda's own state, not only mint a PhysicalResourceId.
Traces: cloudformation__iam__lambda__s3.cfn-lambda-role-getatt-arn-resolution-positive, cloudformation__iam__lambda__s3.cli-cfn-deploy-stack-describe-resources-iam-lambda-positive

Cause 2 of 8: an AWS::IAM::Role declared with a PermissionsBoundary property is created without one.
AWS: the boundary named by the template is attached to the role, so iam:GetRole reports a PermissionsBoundary structure carrying PermissionsBoundaryType and PermissionsBoundaryArn (the boundary policy ARN from the template).
Emulator: GetRole's Role has no PermissionsBoundary member at all (the check fails with KeyError 'PermissionsBoundary').
Why: a client that relies on CFN to enforce a delegation boundary gets an unbounded role — a silent privilege escalation.
Fix: the CloudFormation provisioner for AWS::IAM::Role must pass the template's PermissionsBoundary through to IAM (PutRolePermissionsBoundary semantics) and IAM's role record must report it.
Traces: cloudformation__iam__lambda__s3.cfn-iam-role-permissions-boundary-in-template-positive

Cause 3 of 8: CreateStack with a TemplateURL never authorizes the template fetch, so a stack whose service role cannot read the template object is accepted.
AWS: CloudFormation fetches the TemplateURL object using the RoleARN service role (or the caller when no role is given) and refuses the request when that principal is not granted s3:GetObject on the template object — the call is refused, not accepted.
Emulator: CreateStack answers HTTP 200 with a StackId and OperationId for a service role holding no s3:GetObject on the template object (recorded: StackId 'arn:aws:cloudformation:<region>:<account>:stack/<name>/<uuid>').
Why: a least-privilege deployment that AWS would refuse appears to succeed, so the permission defect is never surfaced to the operator and the negative control silently passes.
Fix: CreateStack's template-fetch path must evaluate the fetching principal's S3 authorization and refuse on denial.
Traces: cloudformation__iam__lambda__s3.cfn-service-role-lacks-s3-getobject-for-template-negative

Cause 4 of 8: lambda:Invoke refuses every real invocation with NotImplemented.
AWS: Invoke runs the function's deployment package and returns 200 with the function's response payload (or a handled FunctionError).
Emulator: Invoke answers NotImplemented with an emulator-authored message 'Invoke resolved <function ARN> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce. InvocationType=DryRun validates an invocation without running the function.' — a code and wording AWS never emits.
Why: no flow that depends on a function actually running can complete, and a client sees an unmodelled error code.
Fix: the Invoke handler's execution path (an in-process runtime for the deployment package).
Traces: cloudformation__iam__lambda__s3.cli-iam-role-create-attach-lambda-execution-positive, cloudformation__iam__lambda__s3.cli-iam-role-deleted-breaks-lambda-invoke-negative

Cause 5 of 8: Invoke's not-found refusal names the unqualified function ARN instead of the resolved qualified ARN.
AWS: invoking a function that has been deleted refuses with ResourceNotFoundException whose message names the ARN Invoke resolved, qualifier included: 'Function not found: arn:aws:lambda:<region>:<account>:function:<name>:$LATEST'. Fixed text 'Function not found: '; the variable part is the resolved ARN including the ':$LATEST' (or explicit) qualifier.
Emulator: the message stops at the unqualified ARN — 'Function not found: arn:aws:lambda:<region>:<account>:function:<name>' — so the ':$LATEST' segment is missing.
Why: a client that parses or compares the refused ARN (or correlates it against the ARN it invoked) does not match AWS's.
Fix: the Invoke handler's ResourceNotFoundException message construction.
Traces: cloudformation__iam__lambda__s3.cfn-delete-stack-removes-iam-role-and-lambda-positive

Cause 6 of 8: an S3 event notification configured for a Lambda target never invokes the function.
AWS: a PutObject into a bucket whose NotificationConfiguration names a Lambda function invokes that function, so the side effect the function performs (here: writing a ProcessedBy object tag back) becomes observable within seconds.
Emulator: nothing is ever invoked — the positive control polls to its ~53 s budget with GetObjectTagging returning [] and the negative control's invocation marker stays None.
Why: every S3-driven pipeline is inert, and a negative control that should observe a *refused* invocation cannot distinguish 'denied' from 'never delivered'.
Fix: S3's object-event notification dispatch path to Lambda targets.
Traces: cloudformation__iam__lambda__s3.s3-notification-triggers-lambda-role-tags-object-positive, cloudformation__iam__lambda__s3.s3-notification-lambda-role-missing-put-tagging-negative

Cause 7 of 8: a cross-account bucket/Lambda notification wiring AWS refuses is accepted.
AWS: the call is refused — the notification cannot be installed when the target's resource policy does not grant s3.amazonaws.com from the bucket's owning account.
Emulator: the call succeeds (the check's `_error_code` helper reports 'expected the call to be refused, but it succeeded').
Why: a misconfiguration AWS rejects at configuration time is accepted, so the operator gets no signal and the notification silently never fires.
Fix: PutBucketNotificationConfiguration's pre-flight validation of the target's permission/account.
Traces: cloudformation__iam__lambda__s3.s3-notification-cross-account-bucket-lambda-role-negative

Cause 8 of 8: a CloudFormation custom resource's provider function is never invoked, so the object the provider writes never exists.
AWS: CFN invokes the custom resource's ServiceToken provider during create, the provider performs its work (writing an S3 object here) and signals SUCCESS, so a subsequent s3:GetObject on that key answers 200.
Emulator: GetObject answers 404 NoSuchKey 'The specified key does not exist.' — the provider never ran. (Related open issue 20261025T180059Z-cloudformation-custom-resource-providers-and-hook-configuration.md describes the same provider-invocation gap but does not carry this replay key.)
Why: every custom-resource-backed stack reports success while producing none of the resource's effects.
Fix: the CloudFormation custom-resource provisioning path (provider invocation + response signalling).
Traces: cloudformation__iam__lambda__s3.cfn-custom-resource-role-least-privilege-positive

## Evidence
Cause 1 — expected: GetFunctionConfiguration on the stack's function answers 200. actual: `ResourceNotFoundException: Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-cfndeploy-fn-<id>` (and the same from GetFunction for cfn-lambda-role-getatt-arn-resolution-positive).
Cause 2 — expected: role['PermissionsBoundary']['PermissionsBoundaryArn'] == the template's boundary ARN. actual: KeyError 'PermissionsBoundary' — the member is absent from GetRole.
Cause 3 — expected: a ClientError refusal. actual: HTTP 200 `{'StackId': 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-xsvc-stk-bad-<id>/<uuid>', 'OperationId': '<uuid>'}`.
Cause 4 — expected: a successful invoke returning the function's payload. actual: `An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:us-west-2:000000000000:function:<name> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator ...`.
Cause 5 — expected message: `Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-fn-<id>:$LATEST`. actual message: `Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-fn-<id>` (':$LATEST' missing).
Cause 6 — expected: the ProcessedBy tag appears on the object. actual: `ProcessedBy tag written back onto the object by the Lambda invocation not met within 52.7s. Last result: []`; negative control: `invocation-marker for the negative-case object not met within 52.7s. Last result: None`.
Cause 7 — expected: refusal. actual: `expected the call to be refused, but it succeeded`.
Cause 8 — expected: the provider-written object readable. actual: `NoSuchKey: An error occurred (NoSuchKey) when calling the GetObject operation: The specified key does not exist.`

## Steps to Reproduce
Replay the behavioral suite and run the cloudformation__iam__lambda__s3 group. Specifically: (1) deploy a stack declaring AWS::IAM::Role + AWS::Lambda::Function and call lambda:GetFunctionConfiguration on the stack's function name; (2) declare AWS::IAM::Role with a PermissionsBoundary and read it back with iam:GetRole; (3) call cloudformation:CreateStack with TemplateURL and a RoleARN whose role has no s3:GetObject on the template object; (4) call lambda:Invoke on any function with a real python3.12 package; (5) delete the function and invoke it, comparing the ResourceNotFoundException message to the resolved ARN; (6) put a bucket NotificationConfiguration naming a Lambda function and upload an object, then poll GetObjectTagging; (7) install a cross-account bucket→Lambda notification; (8) deploy a stack with a Custom:: resource whose provider writes an S3 object, then GetObject that key.
