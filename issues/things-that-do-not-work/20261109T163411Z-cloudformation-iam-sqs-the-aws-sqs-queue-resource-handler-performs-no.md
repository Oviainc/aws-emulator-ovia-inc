Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__sqs.sqs-queue-handler-permission-role-deploys-then-denies-without-create-permission cloudformation__iam__sqs.sqs-queue-handler-permission-update-denied-then-delete-denied-recovers
Issue-Key: cloudformation__iam__sqs.sqs-queue-handler-permission-role-deploys-then-denies-without-create-permission|run#22|AssertionError|assert status == "CREATE_FAILED", f"the role missing sqs:CreateQueue produced {status!r}"
Issue-Key: cloudformation__iam__sqs.sqs-queue-handler-permission-update-denied-then-delete-denied-recovers|run#36|AssertionError|assert status == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# The AWS::SQS::Queue resource handler performs no IAM authorization: stack roles without sqs:CreateQueue or sqs:TagQueue still succeed

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation executes the AWS::SQS::Queue handler without authorizing its calls against the stack's RoleArn, on both create and update.

AWS: CloudFormation calls the resource type's handler as the stack's RoleArn. A role holding every AWS::SQS::Queue handler permission except sqs:CreateQueue cannot create the queue: the resource goes CREATE_FAILED with a ResourceStatusReason carrying the service's AccessDenied (`User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: sqs:CreateQueue on resource: <queue arn> because no identity-based policy allows the sqs:CreateQueue action`) and the stack rolls back. Likewise, an update that adds tags under a role missing sqs:TagQueue fails the resource and the stack reaches UPDATE_ROLLBACK_COMPLETE; restoring the grant lets the next update and the delete succeed. The fixed text of the denial is AWS's standard no-policy wording; principal, action, resource and session come from the request and state. Smithy: sqs#CreateQueue/sqs#TagQueue are authorized operations and the handler-permission list is the resource type's registry schema — recorded behavior only for the stack outcome.

Emulator: the create reaches CREATE_COMPLETE and the tag-adding update reaches UPDATE_COMPLETE with no failed resource events.

Why: a client cannot validate that its stack execution role is least-privilege; a role it believes is restricted provisions and mutates queues regardless, and the recover-after-restore path cannot be exercised at all.

Fix: the CloudFormation resource-provisioning path must authorize each handler call as the stack's RoleArn (the SQS Queue handler's create/update/tag calls), and report the denial as the resource's failure reason.

Traces: cloudformation__iam__sqs.sqs-queue-handler-permission-role-deploys-then-denies-without-create-permission, cloudformation__iam__sqs.sqs-queue-handler-permission-update-denied-then-delete-denied-recovers.

## Evidence
Expected: CREATE_FAILED for the role missing sqs:CreateQueue; UPDATE_ROLLBACK_COMPLETE for the role missing sqs:TagQueue.
Actual: AssertionError: the role missing sqs:CreateQueue produced 'CREATE_COMPLETE'; AssertionError: the role missing sqs:TagQueue produced 'UPDATE_COMPLETE': []

## Steps to Reproduce
1. Create a role trusted by cloudformation.amazonaws.com with the AWS::SQS::Queue handler permissions minus sqs:CreateQueue; CreateStack with that RoleArn and one AWS::SQS::Queue; observe CREATE_COMPLETE.
2. With the grant restored but sqs:TagQueue withheld, UpdateStack to add Tags to the queue; observe UPDATE_COMPLETE instead of UPDATE_ROLLBACK_COMPLETE.
