Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ssm.change-set-import-of-nonexistent-identifier-fails cloudformation__ssm.change-set-import-of-resource-managed-by-another-stack-fails
Issue-Key: cloudformation|CreateChangeSet|ValidationError
Issue-Commit: 486a0852ab6c107011e8dec7ea6574940ef62e53

# cloudformation__ssm: CreateChangeSet on a stack in REVIEW_IN_PROGRESS is refused with ValidationError "can not be updated", so the IMPORT change-set flow cannot continue past its first change set

> _Note: this issue was identified against the code base at commit `486a0852ab6c107011e8dec7ea6574940ef62e53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
REOPENS 20261103T061702Z-cloudformation-ssm-8-net-new-root-causes-in.md: its fix left the call refused.

Cause 1 of 1: a second CreateChangeSet against a stack whose status is REVIEW_IN_PROGRESS is refused instead of being accepted.

AWS: Smithy (cloudformation, awsQuery): CreateChangeSet models only AlreadyExistsException, InsufficientCapabilitiesException and LimitExceededException (all HTTP 400); nothing in the model refuses a call because the target stack is in review, and DescribeStacks' StackStatus enum models REVIEW_IN_PROGRESS as the status of a stack that a change set has created but not executed. Recorded behavior: a stack created by CreateChangeSet (ChangeSetType=CREATE or IMPORT) sits in REVIEW_IN_PROGRESS with no provisioned resources, and further change sets CAN be created on it — including after an earlier change set on that stack settled FAILED. In both recordings the first IMPORT change set settles Status=FAILED (StatusReason `Resource of type 'AWS::SSM::Parameter' with identifier '<name>' was not found.` for a missing identifier, and `<parameter-name> already exists in stack <owner-stack-arn>` for a resource another stack manages — fixed text `Resource of type '<type>' with identifier '<id>' was not found.` and ` already exists in stack `, with the type/identifier taken from ResourcesToImport and the stack ARN from the owning stack's StackId), ExecutionStatus=UNAVAILABLE, the stack stays REVIEW_IN_PROGRESS, and the control leg then creates a SECOND IMPORT change set on that same stack id, which reaches Status=CREATE_COMPLETE with one ResourceChange of Action `Import` (ChangeAction enum includes Import) whose PhysicalResourceId is the imported parameter name.

Emulator: the second CreateChangeSet on that stack id answers HTTP 400 ValidationError with message `Stack:<stack-arn> is in REVIEW_IN_PROGRESS state and can not be updated.` (the ARN is the stack from the first call's StackId). The emulator therefore treats every CreateChangeSet on a REVIEW_IN_PROGRESS stack as an update of a live stack and blocks it; the error code, the HTTP status and the message are all absent from AWS's behavior for this call.

Why: a client that uses the change-set review flow — create a change set for a new stack, inspect it, create another when the first is unusable — is locked out after one attempt: the stack name/id is occupied by a REVIEW_IN_PROGRESS stack that can no longer receive a change set, so resource import can never be retried or corrected, and the only escape is deleting the stack. Both import-refusal checks also lose their positive control, so the emulator's Import classification is never reachable.

Fix: the CreateChangeSet handler's stack-state guard (the "state ... can not be updated" precondition must not fire for StackStatus REVIEW_IN_PROGRESS; a stack in review accepts additional CREATE/IMPORT change sets, including after a FAILED one) and the change-set planning path it gates.

Traces: cloudformation__ssm.change-set-import-of-nonexistent-identifier-fails, cloudformation__ssm.change-set-import-of-resource-managed-by-another-stack-fails.

## Evidence
Expected (AWS, both checks): after the first IMPORT change set settles FAILED on a REVIEW_IN_PROGRESS stack, CreateChangeSet(StackName=<that stack id>, ChangeSetType=IMPORT, ResourcesToImport=[{ResourceType: AWS::SSM::Parameter, LogicalResourceId: P, ResourceIdentifier: {Name: <an importable parameter>}}]) succeeds (HTTP 200 with Id/StackId) and the change set reaches Status=CREATE_COMPLETE with ResourceChange {Action: 'Import', PhysicalResourceId: <the parameter name>}.

Actual (emulator), cloudformation__ssm.change-set-import-of-nonexistent-identifier-fails: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateChangeSet operation: Stack:arn:aws:cloudformation:<region>:<account>:stack/<stack-name>/<uuid> is in REVIEW_IN_PROGRESS state and can not be updated.

Actual (emulator), cloudformation__ssm.change-set-import-of-resource-managed-by-another-stack-fails: the identical refusal — An error occurred (ValidationError) when calling the CreateChangeSet operation: Stack:arn:aws:cloudformation:<region>:<account>:stack/<stack-name>/<uuid> is in REVIEW_IN_PROGRESS state and can not be updated.

In both runs the earlier assertions passed: the first IMPORT change set did settle FAILED with the exact StatusReason, ExecutionStatus UNAVAILABLE, nothing was created, and DescribeStacks reported REVIEW_IN_PROGRESS — the divergence is solely the refusal of the next CreateChangeSet on that stack.

## Steps to Reproduce
1. PutParameter(Name=/<present>, Type=String, Value=existing).
2. CreateChangeSet(StackName=<new name>, ChangeSetType=IMPORT, ChangeSetName=cs1, TemplateBody={Resources:{P:{Type:AWS::SSM::Parameter, DeletionPolicy:Retain, Properties:{Name:/<missing>, Type:String, Value:existing}}}}, ResourcesToImport=[{ResourceType:AWS::SSM::Parameter, LogicalResourceId:P, ResourceIdentifier:{Name:/<missing>}}]) and keep the returned StackId.
3. Poll DescribeChangeSet until FAILED (StatusReason "Resource of type 'AWS::SSM::Parameter' with identifier '/<missing>' was not found."); DescribeStacks(StackName=<StackId>) reports REVIEW_IN_PROGRESS.
4. CreateChangeSet(StackName=<that StackId>, ChangeSetType=IMPORT, ChangeSetName=cs2, TemplateBody naming /<present>, ResourcesToImport=[{... Name:/<present>}]) — the emulator answers ValidationError "Stack:<arn> is in REVIEW_IN_PROGRESS state and can not be updated."; AWS returns 200 and the change set becomes CREATE_COMPLETE with ResourceChange.Action=Import.
5. The same sequence with an owned parameter (step 2's identifier managed by another CREATE_COMPLETE stack) reproduces it identically.
Replay checks cloudformation__ssm.change-set-import-of-nonexistent-identifier-fails and cloudformation__ssm.change-set-import-of-resource-managed-by-another-stack-fails.
