Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.cfn-lambda-role-getatt-arn-resolution-positive cloudformation__iam__lambda__s3.cli-cfn-deploy-stack-describe-resources-iam-lambda-positive cloudformation__iam__lambda__s3.cfn-iam-role-permissions-boundary-in-template-positive cloudformation__iam__lambda__s3.cli-iam-role-create-attach-lambda-execution-positive cloudformation__iam__lambda__s3.cli-iam-role-deleted-breaks-lambda-invoke-negative cloudformation__iam__lambda__s3.s3-notification-triggers-lambda-role-tags-object-positive cloudformation__iam__lambda__s3.s3-notification-lambda-role-missing-put-tagging-negative cloudformation__iam__lambda__s3.s3-notification-cross-account-bucket-lambda-role-negative cloudformation__iam__lambda__s3.cfn-custom-resource-role-least-privilege-positive
Issue-Key: cloudformation__iam__lambda__s3|GetFunction|ResourceNotFoundException
Issue-Key: cloudformation__iam__lambda__s3.cli-cfn-deploy-stack-describe-resources-iam-lambda-positive|_run_shell#20|AssertionError|raise AssertionError(
Issue-Key: cloudformation__iam__lambda__s3.cfn-iam-role-permissions-boundary-in-template-positive|run#7|KeyError|assert role["PermissionsBoundary"]["PermissionsBoundaryArn"] == _BOUNDARY_ARN, (
Issue-Key: cloudformation__iam__lambda__s3.cli-iam-role-create-attach-lambda-execution-positive|_run_shell#22|AssertionError|raise AssertionError(
Issue-Key: cloudformation__iam__lambda__s3.cli-iam-role-deleted-breaks-lambda-invoke-negative|_invoke#19|AssertionError|assert proc.returncode == 0, f"expected successful invoke, got: {proc.stderr[-500:]}"
Issue-Key: cloudformation__iam__lambda__s3.s3-notification-triggers-lambda-role-tags-object-positive|_poll_until#15|AssertionError|raise AssertionError(
Issue-Key: cloudformation__iam__lambda__s3.s3-notification-lambda-role-missing-put-tagging-negative|_poll_until#15|AssertionError|raise AssertionError(
Issue-Key: cloudformation__iam__lambda__s3.s3-notification-cross-account-bucket-lambda-role-negative|_error_code#15|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Key: cloudformation__iam__lambda__s3|GetObject|NoSuchKey
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# cloudformation__iam__lambda__s3: six remaining root causes — the CFN AWS::Lambda::Function provisioner, the PermissionsBoundary it blocks, and everything that needs a function to RUN

> These are the six causes of `20261029T192356Z-cloudformation-iam-lambda-s3-8-net-new-root.md`
> that its fix did NOT close, re-filed with their replay keys so nothing is lost and nothing is
> reported as working. The two it did close (the `TemplateURL` refusal wording and `Invoke`'s
> qualified not-found ARN) are recorded under `issues/things-that-now-work/` with that file name.

## Operation
CreateStack

## Description

Cause 1 of 6 (cause 1 of the original eight): an `AWS::Lambda::Function` declared by a stack
that reports CREATE_COMPLETE does not exist in Lambda.
AWS: CloudFormation creates the modelled resource before the stack reaches CREATE_COMPLETE, so
`lambda:GetFunction` / `GetFunctionConfiguration` on the function named by the stack's
PhysicalResourceId (and by an `Fn::GetAtt` `Arn` output) answers 200 with that function's
configuration.
Emulator: both answer 404 `ResourceNotFoundException` for the exact function the stack declared
and reported created.
Fix: a `AWS::Lambda::Function` arm in the CloudFormation provisioner — `CreateFunction` /
`DeleteFunction`, the readiness wait Lambda's `Pending` state owes, drift off
`GetFunctionConfiguration`, and the deployment package CloudFormation builds from an inline
`Code.ZipFile` (that property is the function's SOURCE; `FunctionCode$ZipFile` is an archive).
Two things block it today, and both are work of their own:
* `services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs` is
  28,707 lines, over the 5,000-line ceiling, so it must be split into a module directory before
  any arm can be added to it;
* nine suites of that crate declare `AWS::Lambda::Function` with NO `Code` property — invalid
  for any real deployment, harmless only while nothing is created — so their fixtures have to be
  reworked in the same change (`bucket_notification_configuration`, `generated_physical_names`,
  `sam_deployed_resources`, `sam_policies_shorthand`, `sam_transform`, `stack_outputs` among
  them).
Related open issue: `20261023T004328Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md`.
Traces: cfn-lambda-role-getatt-arn-resolution-positive, cli-cfn-deploy-stack-describe-resources-iam-lambda-positive

Cause 2 of 6 (cause 2 of the original eight): an `AWS::IAM::Role` declared with a
`PermissionsBoundary` is created without one.
AWS: the boundary named by the template is attached to the role, so `iam:GetRole` reports a
`PermissionsBoundary` structure carrying `PermissionsBoundaryType` and the boundary policy ARN.
Emulator: `GetRole`'s `Role` carries no `PermissionsBoundary` member at all.
Fix: `state/provisioner.rs::create_role` must send `CreateRole`'s own `permissions_boundary`
member — the sibling `create_iam_user` already does — and a changed or dropped boundary must
reach IAM through `PutRolePermissionsBoundary` / `DeleteRolePermissionsBoundary`, with
`PermissionsBoundary` added to `ROLE_IN_PLACE_PROPERTIES`. The change is a dozen lines and was
verified against IAM's own wire, but it lands in the same 28,707-line file as cause 1: that file
must be split first (the merge gate refuses any fix that touches it). Fix it with cause 1, after
the split.
Why: a client that relies on CloudFormation to enforce a delegation boundary gets an unbounded
role — a silent privilege escalation.
Traces: cfn-iam-role-permissions-boundary-in-template-positive (its own assertion is the
`PermissionsBoundary` KeyError; the unit also needs cause 1)

Cause 3 of 6 (cause 4 of the original eight): `lambda:Invoke` refuses every real invocation with
NotImplemented.
AWS: `Invoke` runs the function's deployment package and returns 200 with the function's
response payload (or a handled `FunctionError`).
Emulator: `RequestResponse` answers 501, because there is no in-process runtime for a deployment
package.
Fix: the Invoke execution path — an in-process, contained, bounded runtime for the package, which
is what the code-execution SOP describes. Related open issue:
`20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`.
Traces: cli-iam-role-create-attach-lambda-execution-positive, cli-iam-role-deleted-breaks-lambda-invoke-negative

Cause 4 of 6 (cause 6 of the original eight): an S3 event notification configured for a Lambda
target never has its side effects observed.
AWS: a `PutObject` into a bucket whose `NotificationConfiguration` names a function invokes it,
so what the function does (here: writing a `ProcessedBy` object tag back) becomes observable
within seconds.
Emulator: the delivery itself IS wired — `services/service-com-amazonaws-s3-amazons3/src/state/notifications.rs`
resolves the target and invokes it asynchronously — so what is missing is only the RUN (cause 3):
the queued invocation never executes the package, so no side effect appears.
Traces: s3-notification-triggers-lambda-role-tags-object-positive, s3-notification-lambda-role-missing-put-tagging-negative

Cause 5 of 6 (cause 7 of the original eight): the cross-account bucket/Lambda notification check.
AWS: the call is refused — the notification cannot be installed when the target's resource policy
does not grant `s3.amazonaws.com` from the bucket's owning account.
Emulator: the refusal IS in place (`PutBucketNotificationConfiguration` validates the target's
own policy before recording anything), and the recorded check gets past that assertion. What it
then fails on is the invocation marker it polls for once the permission is corrected — cause 3
again.
Traces: s3-notification-cross-account-bucket-lambda-role-negative

Cause 6 of 6 (cause 8 of the original eight): a CloudFormation custom resource's provider
function is never invoked, so the object the provider writes never exists.
AWS: CloudFormation invokes the custom resource's `ServiceToken` provider during create, the
provider does its work (writing an S3 object here) and signals SUCCESS, so a later
`s3:GetObject` on that key answers 200.
Emulator: `GetObject` answers 404 `NoSuchKey` — the provider never ran, which is cause 3 reached
through the custom-resource path. Related open issue:
`20261025T180059Z-cloudformation-custom-resource-providers-and-hook-configuration.md`.
Traces: cfn-custom-resource-role-least-privilege-positive

## Evidence
Cause 1 — `ResourceNotFoundException: Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-cfndeploy-fn-<id>` from `GetFunctionConfiguration` for a function the stack reported created.
Cause 2 — `KeyError: 'PermissionsBoundary'` on the role `GetRole` reports.
Cause 3 — `An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved <function ARN> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator …`.
Cause 4 — `ProcessedBy tag written back onto the object by the Lambda invocation not met within 52.7s. Last result: []`.
Cause 5 — `invocation marker after the permission was corrected not met within 52.7s. Last result: None`.
Cause 6 — `NoSuchKey … The specified key does not exist.`

## Steps to Reproduce
Replay the behavioral suite and run the cloudformation__iam__lambda__s3 group: (1) deploy a stack
declaring `AWS::IAM::Role` + `AWS::Lambda::Function` and call
`lambda:GetFunctionConfiguration` on the stack's function; (2) declare an `AWS::IAM::Role` with a
`PermissionsBoundary` and read it back with `iam:GetRole`; (3) call `lambda:Invoke` on a function
with a real python3.12 package; (4) put a bucket `NotificationConfiguration` naming a function,
upload an object and poll `GetObjectTagging`; (5) install a cross-account bucket→Lambda
notification, correct the permission and poll for the marker; (6) deploy a stack with a
`Custom::` resource whose provider writes an S3 object, then `GetObject` that key.
