Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied
Issue-Key: cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied|run#44|AssertionError|assert status == "UPDATE_COMPLETE", f"trust update ended {status!r}, expected UPDATE_COMPLETE"
Issue-Commit: 2e55e3f174321d59189949bdc160ce3c5e1a8b8d

# cloudformation: a stack update that changes an AWS::IAM::Role's AssumeRolePolicyDocument ends UPDATE_FAILED — there is no in-place update path for AWS::IAM::Role

> _Note: this issue was identified against the code base at commit `2e55e3f174321d59189949bdc160ce3c5e1a8b8d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
One net-new root cause (Cause 1 of 1) in the bootstrapped `cloudformation` service, grounded in a failing behavioral check the replay report marks neither ALREADY FILED nor OWNED.

Cause 1 of 1: a CloudFormation stack update that changes only an existing AWS::IAM::Role's trust policy leaves the stack in UPDATE_FAILED instead of UPDATE_COMPLETE, and the trust policy is never applied.

AWS: UpdateStack (Smithy com.amazonaws.cloudformation#UpdateStack, input StackName/TemplateBody/Capabilities/Parameters, output UpdateStackOutput{StackId}; errors InsufficientCapabilitiesException, TokenAlreadyExistsException, LimitExceededException) deploys the new template, and AssumeRolePolicyDocument is a NO-INTERRUPTION (in-place) update property of AWS::IAM::Role: CloudFormation applies the change through IAM's own UpdateAssumeRolePolicy (Smithy com.amazonaws.iam#UpdateAssumeRolePolicy, required RoleName + PolicyDocument, modeled errors NoSuchEntity 404 / MalformedPolicyDocument 400 / UnmodifiableEntity 400 / LimitExceeded 409 / ServiceFailure 500). The role therefore keeps its identity — same RoleName, same RoleId, same PhysicalResourceId — and DescribeStacks reports StackStatus `UPDATE_COMPLETE`. Only a change to RoleName is a replacement, after which the old role is deleted and the new one has a different RoleId. (Recorded behavior for the propagation delay the check tolerates; the in-place/replacement classification and the IAM call are AWS's documented resource-type contract.)

Emulator: the update reaches a terminal `UPDATE_FAILED`. The cause is in the per-type provisioner table: services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs `Provisioner::update` has arms for SqsQueue, CloudWatchMetricStream, EventsRule, SsmParameter and CognitoUserPoolGroup only and falls through to `_ => Ok(None)`, so `Provisioner::IamRole` has no update path at all (its create and delete arms exist). services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioning.rs `reconcile` turns that `Ok(None)` into `Materialised::failure` with the text "The following resource(s) failed to update: [<LogicalResourceId>]. NotImplemented: this emulator does not change the properties of an existing AWS::IAM::Role in place; the resource was left as it is." and the stack lands in UPDATE_FAILED with the role's trust policy unchanged. The fixed part of that reason is the emulator's own invented prose and the error code `NotImplemented` is, as the file itself says, "not a code any AWS service answers"; the variable parts are the template's logical id and resource type. AWS emits no refusal here at all — the update succeeds.

Why: a client cannot evolve a role's trust policy through its template. Adding or narrowing a cross-account or service principal is the single most common IAM edit a stack performs, and here it both fails and parks the stack in a failed state, so every subsequent template change to that stack is blocked too. Worse, the stack reports a terminal failure while the role still exists with the OLD trust policy, so a pipeline that treats UPDATE_FAILED as "nothing changed" is right by accident and one that retries never converges.

Fix: the AWS::IAM::Role arm of the per-type provisioner — services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs, `Provisioner::update` — must map a changed AssumeRolePolicyDocument onto IAM UpdateAssumeRolePolicy (and the other in-place properties: Description -> UpdateRole, Policies -> PutRolePolicy/DeleteRolePolicy, ManagedPolicyArns -> AttachRolePolicy/DetachRolePolicy, Tags -> TagRole/UntagRole, MaxSessionDuration -> UpdateRole) over the in-process `iam_client`, returning `Ok(Some(..))` so `reconcile` records the declared configuration and the stack reaches UPDATE_COMPLETE. A RoleName change must keep going down the replacement path it already takes.

Traces: cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied. The SAME root cause is already described as Cause 4 of the open issue 20261025T000319Z-iam-9-net-new-behavioral-root-causes-cloudformation.md, but that issue's replay set names only iam.deploy-iam-role-cloudformation-stack-update-changes-trust-policy; this cross-service check is a different recorded unit it does not re-replay, and it additionally continues into the RoleName-replacement leg and the stack-1 rollback-deletes-the-created-role leg, so it is recorded here.

## Evidence
Replay output:
  [mismatch] cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied
  File "check_cloudformation__iam__sts_deploy_iam_1_cfn_rollback_and_trust_flip_assume_denied.py", line 130, in run
  assert status == "UPDATE_COMPLETE", f"trust update ended {status!r}, expected UPDATE_COMPLETE"
  AssertionError: trust update ended 'UPDATE_FAILED', expected UPDATE_COMPLETE

What the check had already proved before that line: the CAPABILITY_IAM gate refuses a named role (InsufficientCapabilitiesException, message "Requires capabilities : [CAPABILITY_NAMED_IAM]"), the same template with CAPABILITY_NAMED_IAM reaches CREATE_COMPLETE, iam GetRole reports the role with the caller-account-root trust principal, and sts AssumeRole succeeds with an AssumedRoleId scoped to that RoleId. The only call that diverges is the update that replaces the trust principal with {"Service": "ec2.amazonaws.com"} — AWS: UPDATE_COMPLETE with RoleId unchanged; emulator: UPDATE_FAILED.

Corroborating code in this checkout:
  services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs, `Provisioner::update`: arms for `Self::SqsQueue`, `Self::CloudWatchMetricStream`, `Self::EventsRule`, `Self::SsmParameter`, `Self::CognitoUserPoolGroup`, then `_ => Ok(None)` with the comment "Every other type's in-place update is its own entry here ... until one is written, nothing is asked of the owning service and the deployment says so." `IAM_ROLE => Some(Provisioner::IamRole)` is registered in `provisioner_for`, and `Self::IamRole => delete_role(..)` exists in `delete`, so only the update is missing.
  services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioning.rs, `reconcile`: `Ok(None) => { outcome.failure = Some(format!("The following resource(s) failed to update: [{}]. NotImplemented: this emulator does not change the properties of an existing {} in place; the resource was left as it is.", resource.logical_id, resource.resource_type)); ... break; }`.
  services/service-com-amazonaws-cloudformation-cloudformation/src/ops/update_stack.rs header: "a resource an owning service refuses leaves the stack in `UPDATE_FAILED` with that service's own refusal as the reason."

## Steps to Reproduce
1. Build and boot the emulator from this checkout (cloudformation, iam and sts are all bootstrapped and serving).
2. CreateStack with Capabilities=["CAPABILITY_NAMED_IAM"] and a template whose only resource is:
     Role: {Type: AWS::IAM::Role, Properties: {RoleName: <pinned-name>, AssumeRolePolicyDocument: {Version: '2012-10-17', Statement: [{Effect: Allow, Principal: {AWS: 'arn:aws:iam::<account>:root'}, Action: 'sts:AssumeRole'}]}}}
   Wait for CREATE_COMPLETE; iam GetRole(RoleName=<pinned-name>) reports the role and its RoleId.
3. UpdateStack on the same stack with the SAME template except the trust principal, now {"Service": "ec2.amazonaws.com"}, Capabilities=["CAPABILITY_NAMED_IAM"].
4. Poll DescribeStacks. Expect StackStatus `UPDATE_COMPLETE`, iam GetRole reporting the NEW trust principal with the SAME RoleId, and sts AssumeRole by the caller then being refused AccessDenied. Observe StackStatus `UPDATE_FAILED` with reason "The following resource(s) failed to update: [Role]. NotImplemented: this emulator does not change the properties of an existing AWS::IAM::Role in place; the resource was left as it is.", and GetRole still reporting the ORIGINAL account-root principal.
5. Equivalently: re-replay cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied.
