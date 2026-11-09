Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam.cfn-iam-oidc-provider-properties-and-getatt-read-back-then-deleted cloudformation__iam.cfn-iam-saml-provider-metadata-and-getatt-read-back-then-deleted cloudformation__iam.cfn-iam-server-certificate-body-and-getatt-read-back-then-deleted cloudformation__iam.cfn-iam-server-certificate-mismatched-private-key-rejected-and-rolled-back cloudformation__iam.cfn-iam-service-linked-role-autoscaling-suffix-read-back-then-deleted cloudformation__iam.cfn-iam-managed-policy-duplicate-name-rejected-leaving-the-original cloudformation__iam.cfn-iam-user-group-policies-and-virtual-mfa-device-read-back-then-removed-on-delete cloudformation__iam.iam-role-missing-trust-policy-action-rejected
Issue-Key: cloudformation__iam|CreateStack|InsufficientCapabilitiesException
Issue-Key: cloudformation__iam.cfn-iam-managed-policy-duplicate-name-rejected-leaving-the-original|run#17|AssertionError|assert len(failed) == 1, f"expected exactly one stack-level CREATE_FAILED event, got {failed!r}"
Issue-Key: cloudformation__iam.cfn-iam-user-group-policies-and-virtual-mfa-device-read-back-then-removed-on-delete|run#15|AssertionError|assert created == "CREATE_COMPLETE", (
Issue-Key: cloudformation__iam.iam-role-missing-trust-policy-action-rejected|run#8|AssertionError|assert status == "ROLLBACK_COMPLETE", f"expected ROLLBACK_COMPLETE, got {status!r}"
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# cloudformation__iam: 4 net-new causes — CAPABILITY_IAM demanded for non-permission IAM types, no stack-level CREATE_FAILED event on a duplicate policy name, AWS::IAM::Policy on a group fails, and a trust policy with no Action is accepted

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Four distinct root causes in the CloudFormation IAM resource surface.

Cause 1 of 4: CreateStack demands CAPABILITY_IAM for IAM resource types that AWS does not treat as permission-affecting.
AWS: CloudFormation requires CAPABILITY_IAM (or CAPABILITY_NAMED_IAM) only for the IAM resource types that create or grant permissions — AccessKey, Group, InstanceProfile, ManagedPolicy, Policy, Role, User, UserToGroupAddition. AWS::IAM::OIDCProvider, AWS::IAM::SAMLProvider, AWS::IAM::ServerCertificate and AWS::IAM::ServiceLinkedRole are not in that set, so a stack whose only IAM resources are those is created with no Capabilities argument at all. Smithy: cloudformation#CreateStack.Capabilities is an optional list and InsufficientCapabilitiesException is a modelled error of CreateStack; which types require it is a service fact — recorded behavior only, and the recording shows these four stacks created without Capabilities.
Emulator: CreateStack answers 400 `InsufficientCapabilitiesException` with message `Requires capabilities : [CAPABILITY_IAM]` (fixed text `Requires capabilities : [` + `]`, the bracketed list being the capabilities demanded).
Why: a client following AWS's documented capability rules cannot create these stacks at all, and a client that works around it by always passing CAPABILITY_IAM loses the guardrail's meaning.
Fix: the CreateStack/UpdateStack/CreateChangeSet capability gate must apply the permission-affecting type list rather than matching every AWS::IAM::* type.
Traces: cloudformation__iam.cfn-iam-oidc-provider-properties-and-getatt-read-back-then-deleted, cloudformation__iam.cfn-iam-saml-provider-metadata-and-getatt-read-back-then-deleted, cloudformation__iam.cfn-iam-server-certificate-body-and-getatt-read-back-then-deleted, cloudformation__iam.cfn-iam-server-certificate-mismatched-private-key-rejected-and-rolled-back, cloudformation__iam.cfn-iam-service-linked-role-autoscaling-suffix-read-back-then-deleted.

Cause 2 of 4: a stack that fails because a managed-policy name is already taken emits no stack-level CREATE_FAILED event.
AWS: when AWS::IAM::ManagedPolicy create fails on an EntityAlreadyExists, DescribeStackEvents carries exactly one event whose LogicalResourceId is the stack itself with ResourceStatus CREATE_FAILED and a ResourceStatusReason naming the failure, alongside the per-resource CREATE_FAILED event; the pre-existing policy is left untouched. Smithy: cloudformation#DescribeStackEvents returns StackEvents[].ResourceStatus — recorded behavior only for the event set.
Emulator: the stack-level CREATE_FAILED event is absent (the filtered event list is empty), so a client polling events cannot see why the stack failed.
Why: tooling that reports deployment failures from stack events (the CLI, CDK, Serverless) reports no reason at all.
Fix: the stack-event emitter in the CloudFormation engine must publish the stack-level CREATE_FAILED event with its reason on rollback.
Traces: cloudformation__iam.cfn-iam-managed-policy-duplicate-name-rejected-leaving-the-original.

Cause 3 of 4: AWS::IAM::Policy attached to a stack-declared group fails to create, rolling the stack back.
AWS: a stack declaring an AWS::IAM::User, an AWS::IAM::Group, a user inline policy and a group inline policy (AWS::IAM::Policy with Groups: [<group>]) reaches CREATE_COMPLETE; the policies are readable with GetUserPolicy/GetGroupPolicy and are removed on DeleteStack. Smithy: iam#PutGroupPolicy takes GroupName + PolicyName + PolicyDocument — recorded behavior only for the stack outcome.
Emulator: the GroupPolicy resource goes CREATE_FAILED and the stack ends ROLLBACK_COMPLETE (event order: User/Group/UserPolicy CREATE_COMPLETE, GroupPolicy CREATE_FAILED, then deletes).
Why: a client cannot deploy the most common IAM shape of all — a group with an inline policy — so no stack mixing users, groups and inline policies deploys.
Fix: the AWS::IAM::Policy provisioner's group branch (its call into PutGroupPolicy) inside the CloudFormation IAM handlers.
Traces: cloudformation__iam.cfn-iam-user-group-policies-and-virtual-mfa-device-read-back-then-removed-on-delete.

Cause 4 of 4: an AWS::IAM::Role whose AssumeRolePolicyDocument statement has no Action is accepted.
AWS: IAM refuses a trust policy statement with no Action with MalformedPolicyDocument, so the stack resource goes CREATE_FAILED and the stack ends ROLLBACK_COMPLETE. Smithy: iam#CreateRole declares MalformedPolicyDocumentException for AssumeRolePolicyDocument — recorded behavior only for the stack status.
Emulator: the stack reaches CREATE_COMPLETE, i.e. the role is created with an actionless trust policy.
Why: a client relying on CloudFormation to reject malformed trust policies ships a role that can never be assumed and learns nothing at deploy time.
Fix: trust-document validation in the IAM CreateRole/UpdateAssumeRolePolicy validation path, surfaced through the AWS::IAM::Role provisioner.
Traces: cloudformation__iam.iam-role-missing-trust-policy-action-rejected.

## Evidence
Cause 1 — Expected: CreateStack accepted with no Capabilities for OIDCProvider / SAMLProvider / ServerCertificate / ServiceLinkedRole stacks. Actual: botocore.errorfactory.InsufficientCapabilitiesException: An error occurred (InsufficientCapabilitiesException) when calling the CreateStack operation: Requires capabilities : [CAPABILITY_IAM]
Cause 2 — Expected: exactly one stack-level CREATE_FAILED event. Actual: AssertionError: expected exactly one stack-level CREATE_FAILED event, got []
Cause 3 — Expected: CREATE_COMPLETE. Actual: AssertionError: expected CREATE_COMPLETE, got 'ROLLBACK_COMPLETE': [('User','CREATE_COMPLETE'),('Group','CREATE_COMPLETE'),('UserPolicy','CREATE_COMPLETE'),('GroupPolicy','CREATE_FAILED'),…]
Cause 4 — Expected: ROLLBACK_COMPLETE. Actual: AssertionError: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE'

## Steps to Reproduce
Cause 1: CreateStack (no Capabilities) with a template whose only IAM resource is AWS::IAM::OIDCProvider (then SAMLProvider, ServerCertificate, ServiceLinkedRole); observe InsufficientCapabilitiesException.
Cause 2: create a managed policy, then CreateStack declaring AWS::IAM::ManagedPolicy with the same ManagedPolicyName; poll DescribeStackEvents for a stack-level CREATE_FAILED event.
Cause 3: CreateStack with AWS::IAM::User, AWS::IAM::Group and an AWS::IAM::Policy whose Groups names the group; observe GroupPolicy CREATE_FAILED.
Cause 4: CreateStack with an AWS::IAM::Role whose AssumeRolePolicyDocument statement omits Action; observe CREATE_COMPLETE.
