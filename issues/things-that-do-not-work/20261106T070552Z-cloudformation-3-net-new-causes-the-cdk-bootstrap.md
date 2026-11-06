Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cdk-default-synthesizer-needs-bootstrap-without-user-assets cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete cloudformation.force-delete-stack-rejected-unless-delete-failed cloudformation.import-existing-resources-adopts-named-parameter
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cdk-default-synthesizer-needs-bootstrap-without-user-assets|_cdk#40|AssertionError|assert result.returncode == expect_rc, (
Issue-Key: cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete|_cdk#13|AssertionError|assert result.returncode == expect_rc, (
Issue-Key: cloudformation.force-delete-stack-rejected-unless-delete-failed|run#11|AssertionError|assert message == (
Issue-Key: cloudformation.import-existing-resources-adopts-named-parameter|run#17|AssertionError|assert desc["Status"] == "CREATE_COMPLETE", f"change set: expected CREATE_COMPLETE, got {desc!r}"
Issue-Commit: 0de1c688e6ce96ac9f39fea8e8ed5bb261aa89c9

# cloudformation: 3 net-new causes — the cdk bootstrap file-publishing role is denied s3:ListBucket on its own assets bucket, FORCE_DELETE_STACK's refusal message is reworded, and CreateChangeSet ignores ImportExistingResources

> _Note: this issue was identified against the code base at commit `0de1c688e6ce96ac9f39fea8e8ed5bb261aa89c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Three independent root causes, all grounded in failing behavioral checks listed under `### cloudformation`. A fixer must close all three before submitting.

Cause 1 of 3: the `cdk bootstrap`-created file-publishing role is refused s3:ListBucket on the CDK assets bucket, so no `cdk deploy` ever reaches CloudFormation.
AWS: recorded behavior only (no Smithy model describes IAM authorization). `cdk bootstrap` creates the CDKToolkit stack, whose `cdk-<qualifier>-file-publishing-role-<account>-<region>` role is attached a policy granting the asset bucket actions (s3:GetObject*/GetBucket*/List*/Abort*/PutObject*) on `arn:<partition>:s3:::cdk-<qualifier>-assets-<account>-<region>` and its objects. After the CLI assumes that role, its ListObjectsV2/ListBucket preflight on the assets bucket is authorized and the stack template asset publishes; `cdk deploy` then creates the stack and exits 0.
Emulator: the assumed role is denied — `User: arn:aws:sts::<account>:assumed-role/cdk-<qualifier>-file-publishing-role-<account>-<region>/aws-cdk-mockforge is not authorized to perform: s3:ListBucket on resource: "arn:aws:s3:::cdk-<qualifier>-assets-<account>-<region>" because no identity-based policy allows the s3:ListBucket action`. The CLI prints `Failed to publish asset <stack> Template (<account>-<region>-<hash>)` followed by `AggregateError:` and exits 1, before any CreateChangeSet/CreateStack call is made.
Why: every CDK-authored deployment against the emulator aborts at asset publication, so no CDK contract (bootstrap requirement for the default synthesizer, nested-stack tracking and cascade on destroy) can be exercised at all. A client that bootstraps and deploys exactly as documented cannot deploy.
Fix: the CDKToolkit bootstrap template's file-publishing-role policy as the emulator provisions it (the asset-bucket resource statements it attaches), and/or the IAM authorization evaluation of s3:ListBucket against a bucket ARN for an assumed role.
Traces: cloudformation.cdk-default-synthesizer-needs-bootstrap-without-user-assets, cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete. The same denial is the cause of the separately filed cloudformation__ec2, cloudformation__ecs and cloudformation__iam__lambda__s3 issues from this sweep.

Cause 2 of 3: DeleteStack's refusal of DeletionMode=FORCE_DELETE_STACK outside DELETE_FAILED carries the wrong message.
AWS: Smithy: DeleteStack.DeletionMode targets enum DeletionMode with values [STANDARD, FORCE_DELETE_STACK]; the model declares no modeled error for this, so the refusal is the service-common 400 ValidationError. Its message is `Invalid operation on stack [<stackId>]. You can activate DeletionMode FORCE_DELETE_STACK in a delete stack operation only when the stack is in the DELETE_FAILED state.` Everything outside the brackets is fixed text; the only variable part is the stack id taken from the request/state.
Emulator: the status and error code are right (400 ValidationError) but the message is reworded to `Stack [<stackArn>] cannot be deleted with DeletionMode FORCE_DELETE_STACK while it is in <StackStatus> state` — a different sentence that also drops the statement of the one state in which the mode is permitted.
Why: a client or operator matching the documented sentence (or reading it to learn when FORCE_DELETE_STACK is usable) gets neither.
Fix: DeleteStack's handler validation of DeletionMode.
Traces: cloudformation.force-delete-stack-rejected-unless-delete-failed.

Cause 3 of 3: CreateChangeSet ignores ImportExistingResources — the name-conflict early validation fires and the change set FAILs instead of adopting the existing resource.
AWS: Smithy: CreateChangeSet.ImportExistingResources is a boolean input member. With it true and ChangeSetType CREATE, a template declaring a resource whose named physical resource already exists out of band settles the change set at Status CREATE_COMPLETE / ExecutionStatus AVAILABLE, and the Change for that resource carries ResourceChange.Action `Import`; executing it adopts the resource without rewriting its value.
Emulator: the change set settles Status `FAILED`, ExecutionStatus `UNAVAILABLE`, StatusReason `The following hook(s)/validation failed: [AWS::EarlyValidation::ResourceExistenceCheck]. To troubleshoot Early Validation errors, use the DescribeEvents API for detailed failure information.`, and its single Change carries Action `Add`, not `Import` — the flag is dropped on the floor and the existence check is applied as if it were absent.
Why: automatic resource import is unusable: no caller can bring an existing resource under stack management, and every `--import-existing-resources` deploy fails on a name that AWS would adopt.
Fix: CreateChangeSet's handler — ImportExistingResources must suppress the AWS::EarlyValidation::ResourceExistenceCheck name-conflict gate for the matched resources and record those changes as Action Import.
Traces: cloudformation.import-existing-resources-adopts-named-parameter. The same cause under a different service heading is filed separately for cloudformation__ssm.

## Evidence
Cause 1: `cdk deploy <stack> --require-approval never` exits 1; output tail — `<stack>: fail: User: arn:aws:sts::000000000000:assumed-role/cdk-qaf44255ec-file-publishing-role-000000000000-us-west-2/aws-cdk-mockforge is not authorized to perform: s3:ListBucket on resource: "arn:aws:s3:::cdk-qaf44255ec-assets-000000000000-us-west-2" because no identity-based policy allows the s3:ListBucket action` / `Failed to publish asset <stack> Template (...)` / `AggregateError:`. Expected rc 0 and a created stack (bucket/qualifier/host values shown are per-run).

Cause 2: DeleteStack(StackName=<id>, DeletionMode='FORCE_DELETE_STACK') on a CREATE_COMPLETE stack.
Expected: ('ValidationError', 'Invalid operation on stack [<stackId>]. You can activate DeletionMode FORCE_DELETE_STACK in a delete stack operation only when the stack is in the DELETE_FAILED state.')
Actual:   ('ValidationError', 'Stack [arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-cfn-force-4a09a681036f/8e6b0722-...] cannot be deleted with DeletionMode FORCE_DELETE_STACK while it is in CREATE_COMPLETE state')

Cause 3: CreateChangeSet(ChangeSetType='CREATE', ImportExistingResources=True) over a template declaring an AWS::SSM::Parameter whose name already exists.
Expected: DescribeChangeSet Status 'CREATE_COMPLETE', ExecutionStatus 'AVAILABLE', Changes[0].ResourceChange.Action 'Import'.
Actual:   Status 'FAILED', ExecutionStatus 'UNAVAILABLE', StatusReason 'The following hook(s)/validation failed: [AWS::EarlyValidation::ResourceExistenceCheck]. To troubleshoot Early Validation errors, use the DescribeEvents API for detailed failure information.', Changes[0].ResourceChange.Action 'Add'.

## Steps to Reproduce
Cause 1: `cdk bootstrap --qualifier <q> --toolkit-stack-name <name>` against the emulator, then `cdk deploy <stack> --require-approval never` for any app (asset-free or not). The deploy exits 1 at the publish step with the s3:ListBucket denial quoted above.
Cause 2: CreateStack any template, wait for CREATE_COMPLETE, then DeleteStack(StackName=<stackId>, DeletionMode='FORCE_DELETE_STACK') and compare Error.Code/Error.Message.
Cause 3: PutParameter a parameter out of band, then CreateChangeSet(StackName=<new>, ChangeSetType='CREATE', ImportExistingResources=True, TemplateBody=<template declaring that parameter name>) and poll DescribeChangeSet.
Replay: cloudformation.cdk-default-synthesizer-needs-bootstrap-without-user-assets, cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete, cloudformation.force-delete-stack-rejected-unless-delete-failed, cloudformation.import-existing-resources-adopts-named-parameter.
