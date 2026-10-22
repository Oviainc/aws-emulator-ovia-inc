Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-change-set-create-and-execute
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-change-set-create-and-execute cloudformation-changeset-create-execute-workflow cloudformation-changeset-lifecycle
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DescribeStacks|2xx->4xx|$
Issue-Key: cloudformation|DescribeStacks|2xx->4xx|$.DescribeStacksResponse
Issue-Key: cloudformation|DescribeStacks|2xx->4xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.StackStatus
Issue-Key: cloudformation|DescribeStacks|2xx->4xx|status
Issue-Key: cloudformation|GetTemplate|2xx->4xx|$.GetTemplateResponse
Issue-Key: cloudformation|GetTemplate|2xx->4xx|status
Issue-Commit: bffba5cba440022693dbdf16c8135b8d2c175737

# CloudFormation change sets never materialize their stack: REVIEW_IN_PROGRESS and post-execute reads fail

> _Note: this issue was identified against the code base at commit `bffba5cba440022693dbdf16c8135b8d2c175737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
FOUR net-new causes, one shared defect: CreateChangeSet with ChangeSetType CREATE does not create the stack record it returns a StackId for, and ExecuteChangeSet does not bring that stack into existence either. Every subsequent read of the stack therefore fails with a 400 'does not exist' instead of returning a Stack.

Cause 1 of 4 — after ExecuteChangeSet, DescribeStacks never reaches CREATE_COMPLETE (fixture cloudformation-change-set-create-and-execute, task#6; cluster also covers cloudformation-changeset-create-execute-workflow#task#5).
AWS: ExecuteChangeSet on a CREATE change set drives the stack to CREATE_COMPLETE, and DescribeStacks(StackName) then returns 200 with a Stack whose StackStatus is CREATE_COMPLETE and whose ChangeSetId is the executed change set. Smithy: CreateChangeSet output {Id: ChangeSetId, StackId}; ExecuteChangeSet is modeled on the same stack resource; DescribeStacks declares NO error for a named stack, and the recorded not-found response is 400 ValidationError.
Emulator: the poll for StackStatus=CREATE_COMPLETE never succeeds — every attempt returns HTTP 400 ValidationError 'Stack with id <name> does not exist' (13 attempts, capped at 60s). Expected 200, got 400.
Why: the whole change-set deployment path is unusable. A client that deploys via change sets (the default for CDK/SAM-style flows) gets a successful CreateChangeSet/ExecuteChangeSet and then cannot find the stack it was told it created, so it cannot wait for completion, read outputs, update, or clean up.
Fix: the CreateChangeSet / ExecuteChangeSet handlers and the stack state they write in the CloudFormation service (services/service-com-amazonaws-cloudformation-cloudformation): CreateChangeSet(ChangeSetType=CREATE) must insert the stack in REVIEW_IN_PROGRESS keyed by the StackId it returns, and ExecuteChangeSet must transition that same record through CREATE_IN_PROGRESS to CREATE_COMPLETE.
Traces: cloudformation-change-set-create-and-execute#task#6.

Cause 2 of 4 — GetTemplate on the change-set-created stack fails (cluster also covers cloudformation-changeset-create-execute-workflow#task#8). AWS: 200 with GetTemplateResult.TemplateBody equal to the change set's template and StagesAvailable [Original, Processed]. Emulator: 400 — no GetTemplateResponse at all, because the stack does not exist. Why: the template of an executed change set cannot be read back, breaking UsePreviousTemplate-style follow-up updates. Fix: as cause 1 (the template must be attached to the stack record on execute). Traces: cloudformation-change-set-create-and-execute#task#7.

Cause 3 of 4 — the stack is not visible in REVIEW_IN_PROGRESS right after CreateChangeSet (cluster also covers cloudformation-changeset-create-execute-workflow#task#7). AWS: immediately after CreateChangeSet(ChangeSetType=CREATE), DescribeStacks returns 200 with StackStatus REVIEW_IN_PROGRESS, StackStatusReason 'User Initiated' and the StackId the change set reported. Emulator: 400, no Stack. Why: clients that inspect or gate on the review state before executing see a non-existent stack. Fix: as cause 1. Traces: cloudformation-changeset-create-execute-workflow#task#2.

Cause 4 of 4 — teardown describe of a change-set stack fails. AWS: 200. Emulator: 400. Why: teardown/verification steps cannot confirm the stack's state at all, so a change-set-based workflow has no observable end state. Fix: as cause 1. Traces: cloudformation-changeset-lifecycle#teardown#11.

## Evidence
Replay of simparity-wire-aws-v1.

Cause 1 — cloudformation-change-set-create-and-execute#task#6, DescribeStacks:
  status: expected 200, got 400
  $: poll capped at 60s; got {'ErrorResponse': {'Error': {'Type': 'Sender', 'Code': 'ValidationError', 'Message': 'Stack with id sim2real-probe-cloudformation-cfn-cs-stack-... does not exist'}}}
  $.DescribeStacksResponse...Stacks.member.StackStatus: poll: not in ['CREATE_COMPLETE'] after 13 attempt(s) (expected 'CREATE_COMPLETE', got None)
  (cluster: cloudformation-changeset-create-execute-workflow#task#5)

Cause 2 — cloudformation-change-set-create-and-execute#task#7, GetTemplate:
  status: expected 200, got 400
  $.GetTemplateResponse: missing (expected GetTemplateResult.TemplateBody '{"AWSTemplateFormatVersion": "2010-09-09", "Resources": {"Bucket": {"Type": "AWS::S3::Bucket"}}}' with StagesAvailable ['Original','Processed'])
  (cluster: cloudformation-changeset-create-execute-workflow#task#8)

Cause 3 — cloudformation-changeset-create-execute-workflow#task#2, DescribeStacks:
  status: expected 200, got 400
  $.DescribeStacksResponse: missing (expected Stacks.member with StackStatus 'REVIEW_IN_PROGRESS', StackStatusReason 'User Initiated', StackId {{changeset_stack_id}})
  (cluster: cloudformation-changeset-create-execute-workflow#task#7)

Cause 4 — cloudformation-changeset-lifecycle#teardown#11, DescribeStacks: status: expected 200, got 400.

Stack names, stack ids and request ids in the expected documents are environment-specific; the requirements asserted are the 200 status, the stack's existence, and the StackStatus values REVIEW_IN_PROGRESS (after create) and CREATE_COMPLETE (after execute).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Call CreateChangeSet(StackName=<new>, ChangeSetName=<name>, ChangeSetType=CREATE, TemplateBody={"Resources":{"Bucket":{"Type":"AWS::S3::Bucket"}}}) — note the returned StackId.
3. Call DescribeStacks(StackName=<new>) immediately (cloudformation-changeset-create-execute-workflow#task#2): the emulator returns 400 ValidationError 'does not exist'; AWS returns 200 with StackStatus REVIEW_IN_PROGRESS / StackStatusReason 'User Initiated'.
4. Call ExecuteChangeSet, then poll DescribeStacks for CREATE_COMPLETE (cloudformation-change-set-create-and-execute#task#6): every poll returns 400 'does not exist' until the 60s cap.
5. Call GetTemplate(StackName=<new>) (cloudformation-change-set-create-and-execute#task#7): 400 instead of 200 with the change set's template.
6. Re-replay the representative plus the three bindings in Issue-Trace-Ids to confirm the fix.
