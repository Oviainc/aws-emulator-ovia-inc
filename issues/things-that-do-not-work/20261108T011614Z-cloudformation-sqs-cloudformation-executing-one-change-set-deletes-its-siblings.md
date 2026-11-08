Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs.cfn-execute-obsolete-changeset-rejected
Issue-Key: cloudformation__sqs|DescribeChangeSet|ChangeSetNotFound
Issue-Commit: 73a8baeac3f8a94bf357d3a10d4553d70859ebf3

# cloudformation: executing one change set DELETES its siblings instead of obsoleting them — DescribeChangeSet/ExecuteChangeSet on the sibling answer ChangeSetNotFound (404) instead of ExecutionStatus OBSOLETE and InvalidChangeSetStatus (400)

> _Note: this issue was identified against the code base at commit `73a8baeac3f8a94bf357d3a10d4553d70859ebf3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
Cause 1 of 1: a stack with several outstanding change sets loses every non-executed one the moment one of them is executed, instead of leaving them queryable in ExecutionStatus OBSOLETE. Failing check: cloudformation__sqs.cfn-execute-obsolete-changeset-rejected.

AWS: executing a change set invalidates the stack's other outstanding change sets but does NOT make them unaddressable. The sibling stays describable and DescribeChangeSet reports ExecutionStatus = OBSOLETE (Smithy: enum ExecutionStatus = UNAVAILABLE | AVAILABLE | EXECUTE_IN_PROGRESS | EXECUTE_COMPLETE | EXECUTE_FAILED | OBSOLETE — OBSOLETE exists precisely for this state and is unreachable if the record is removed). A subsequent ExecuteChangeSet on that obsolete change set is refused with the modelled InvalidChangeSetStatusException (Smithy: errorKind client, httpError 400, awsQueryError code InvalidChangeSetStatus, doc "The specified change set can't be used to update the stack"), with the message template "ChangeSet [<change-set-id>] cannot be executed in its current execution status of [<ExecutionStatus>]" — fixed text is "ChangeSet [", "] cannot be executed in its current execution status of [", "]"; the variable parts are the change set id/ARN taken from the request and the status taken from emulator state (here OBSOLETE). ChangeSetNotFoundException is the other, distinct modelled error (httpError 404, awsQueryError code ChangeSetNotFound, doc "The specified change set name or ID doesn't exit") and is reserved for a change set that was never created or was deleted by DeleteChangeSet. Recorded behavior in the check confirms both: the sibling must read back ExecutionStatus 'OBSOLETE', and executing it must be refused with code InvalidChangeSetStatus and that exact message.

Emulator: ExecuteChangeSet physically purges every other change set of the stack from the store — services/service-com-amazonaws-cloudformation-cloudformation/src/ops/execute_change_set.rs retains only the executed one (`store.change_sets.retain(|other| other.stack_id != stack_id || other.change_set_id == change_set_id)`), reading the AWS doc sentence "CloudFormation deletes all other change sets associated with the stack" as a hard delete of the record. Consequently the next DescribeChangeSet on the sibling raises ChangeSetNotFound: `botocore.errorfactory.ChangeSetNotFoundException: An error occurred (ChangeSetNotFound) when calling the DescribeChangeSet operation: ChangeSet [arn:aws:cloudformation:<region>:<account>:changeSet/<name>/<id>] does not exist` (404), and ExecuteChangeSet on it would answer the same 404/ChangeSetNotFound rather than 400/InvalidChangeSetStatus. The check never reaches its OBSOLETE assertion — it errors out on the describe.

Why: a client that enumerates or reconciles outstanding change sets after a deploy cannot tell an invalidated change set from one that never existed. Tooling that polls DescribeChangeSet for ExecutionStatus (CLI/CDK/custom deploy pipelines) gets a 404 ChangeSetNotFound and treats the change set as deleted or the stack as missing, instead of seeing OBSOLETE and reporting "this plan is stale, create a new change set". Code that retries ExecuteChangeSet on the stale plan gets the wrong error class — a 404 not-found instead of the 400 InvalidChangeSetStatus that signals a status conflict — so retry/branching logic keyed on InvalidChangeSetStatus never fires, and ListChangeSets loses the OBSOLETE rows entirely.

Fix: ExecuteChangeSet's handler and the change-set state it writes. The sibling records must be RETAINED and transitioned to ExecutionStatus OBSOLETE rather than removed from the store, so DescribeChangeSet/ListChangeSets keep reporting them; ExecuteChangeSet's status guard must then classify OBSOLETE as InvalidChangeSetStatusException with the status-conflict message template above (it already produces that error class for CREATE-incomplete/already-executed change sets). Related but not covering it: open issue 20261029T214321Z-cloudformation-the-change-set-status-surface-diverges-delete.md describes other change-set status/refusal-message divergences; it does not mention the sibling records being deleted on execute.

Traces: cloudformation__sqs.cfn-execute-obsolete-changeset-rejected (the check creates change sets A and B on one stack, executes B to UPDATE_COMPLETE, then asserts A reports OBSOLETE and that executing A is refused with InvalidChangeSetStatus); the deleting retain is at services/service-com-amazonaws-cloudformation-cloudformation/src/ops/execute_change_set.rs around line 734.

## Evidence
Expected (AWS / recorded): after ExecuteChangeSet on sibling B completes, DescribeChangeSet(ChangeSetName=<A's id>) returns 200 with ExecutionStatus="OBSOLETE"; ExecuteChangeSet(ChangeSetName=<A's id>) is refused HTTP 400, Error.Code="InvalidChangeSetStatus", Error.Message="ChangeSet [<A's change-set ARN>] cannot be executed in its current execution status of [OBSOLETE]"; the stack still reports UPDATE_COMPLETE and the queue keeps B's VisibilityTimeout.

Actual (emulator): DescribeChangeSet(ChangeSetName=<A's id>) is refused HTTP 404 — botocore.errorfactory.ChangeSetNotFoundException: An error occurred (ChangeSetNotFound) when calling the DescribeChangeSet operation: ChangeSet [arn:aws:cloudformation:us-west-2:000000000000:changeSet/sim2real-xsvc-obs-csa-<suffix>/<uuid>] does not exist. The record is gone, so ExecutionStatus OBSOLETE is never observable and ExecuteChangeSet on A can only answer ChangeSetNotFound (404) instead of InvalidChangeSetStatus (400). Source: execute_change_set.rs — `store.change_sets.retain(|other| other.stack_id != stack_id || other.change_set_id == change_set_id)`.

## Steps to Reproduce
1. CreateStack with a template whose AWS::SQS::Queue takes QueueName and VisibilityTimeout parameters; wait for CREATE_COMPLETE with VisibilityTimeout=30.
2. CreateChangeSet A (ChangeSetType=UPDATE, UsePreviousTemplate=true, Timeout=40) and CreateChangeSet B (same, Timeout=50); keep both Ids. Both report Status=CREATE_COMPLETE, ExecutionStatus=AVAILABLE.
3. ExecuteChangeSet(ChangeSetName=<B's Id>) and wait for the stack to reach UPDATE_COMPLETE.
4. DescribeChangeSet(ChangeSetName=<A's Id>) — emulator answers 404 ChangeSetNotFound ("ChangeSet [<A's ARN>] does not exist"); AWS answers 200 with ExecutionStatus=OBSOLETE.
5. ExecuteChangeSet(ChangeSetName=<A's Id>) — AWS refuses 400 InvalidChangeSetStatus "ChangeSet [<A's ARN>] cannot be executed in its current execution status of [OBSOLETE]"; the emulator cannot produce that because the record was deleted in step 3.
