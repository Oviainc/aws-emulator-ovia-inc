Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__sns.sns-topic-handler-permission-role-deploys-then-denies-without-create-permission
Issue-Key: cloudformation__iam__sns.sns-topic-handler-permission-role-deploys-then-denies-without-create-permission|run#23|AssertionError|assert status == "CREATE_FAILED", f"the role missing sns:CreateTopic produced {status!r}"
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# The AWS::SNS::Topic resource handler performs no IAM authorization: a stack role without sns:CreateTopic still creates the topic

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation executes the AWS::SNS::Topic handler without authorizing the calls against the stack's RoleArn, so a role that lacks the resource type's declared handler permissions still provisions the resource.

AWS: CloudFormation calls the resource type's handler with the credentials of the stack's RoleArn. A role holding every AWS::SNS::Topic create/update/delete handler permission except sns:CreateTopic cannot create the topic: the resource goes CREATE_FAILED with a ResourceStatusReason carrying the service's AccessDenied (`User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: sns:CreateTopic on resource: <topic arn> because no identity-based policy allows the sns:CreateTopic action`) and the stack rolls back. The fixed text of that denial is AWS's standard no-policy wording; the principal ARN, action, resource and session are taken from the request and state. Smithy: sns#CreateTopic declares AuthorizationErrorException; the handler-permission list is the resource type's registry schema — recorded behavior only for the stack outcome.

Emulator: the resource reaches CREATE_COMPLETE — the handler call is never authorized.

Why: least-privilege testing of stack execution roles is impossible: a client cannot verify that its deploy role is correctly scoped, and a role it believes is restricted in fact provisions anything. The negative result is also unsafe to rely on in reverse — a client cannot tell a correct grant from an unenforced one.

Fix: the CloudFormation resource-provisioning path must sign/authorize each handler call as the stack's RoleArn (the SNS Topic handler in particular), and surface the service's denial as the resource's CREATE_FAILED reason.

Traces: cloudformation__iam__sns.sns-topic-handler-permission-role-deploys-then-denies-without-create-permission.

## Evidence
Expected: the AWS::SNS::Topic resource reports CREATE_FAILED when the stack role is missing sns:CreateTopic.
Actual: AssertionError: the role missing sns:CreateTopic produced 'CREATE_COMPLETE'

## Steps to Reproduce
1. Create a role trusted by cloudformation.amazonaws.com holding the AWS::SNS::Topic handler permissions minus sns:CreateTopic.
2. CreateStack with that RoleArn and a single AWS::SNS::Topic resource.
3. Poll DescribeStackEvents; observe CREATE_COMPLETE instead of CREATE_FAILED with an AccessDenied reason.
