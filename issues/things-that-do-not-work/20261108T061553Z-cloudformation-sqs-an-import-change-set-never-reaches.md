Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one
Issue-Key: cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one|_poll_until#29|Timeout|time.sleep(interval_s)
Issue-Commit: 89584cbdc793a0842735dd0ecc1a58bc9d0678bb

# cloudformation__sqs: an IMPORT change set never reaches a terminal state, so `cdk import` of an existing queue (and of a missing one) hangs forever

> _Note: this issue was identified against the code base at commit `89584cbdc793a0842735dd0ecc1a58bc9d0678bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Cause 1 of 1: CloudFormation's resource-import path never reaches a terminal change-set or stack state, so every caller that waits on an import blocks indefinitely.

AWS: a resource import is driven by CreateChangeSet with ChangeSetType=IMPORT and a ResourcesToImport entry per adopted resource (Smithy: cloudformation CreateChangeSet input members ChangeSetType -> enum ChangeSetType {CREATE, UPDATE, IMPORT} and ResourcesToImport, list length 0..200). The change set always settles on a terminal ChangeSetStatus (Smithy: DescribeChangeSet output Status -> enum ChangeSetStatus {CREATE_PENDING, CREATE_IN_PROGRESS, CREATE_COMPLETE, DELETE_PENDING, DELETE_IN_PROGRESS, DELETE_COMPLETE, DELETE_FAILED, FAILED} with StatusReason): when an entry names an object that does not exist the change set ends FAILED with StatusReason "Resource of type '<ResourceType>' with identifier '<the identifier supplied in ResourcesToImport>' was not found." — fixed text whose variable parts are the resource type and the identifier taken verbatim from the request — and the stack is left exactly as it was (here CREATE_COMPLETE); when the entry names a resource that does exist, executing the change set drives the stack IMPORT_IN_PROGRESS -> IMPORT_COMPLETE (Smithy: StackStatus includes IMPORT_IN_PROGRESS, IMPORT_COMPLETE, IMPORT_ROLLBACK_*), the imported resource's StackResourceDetail.PhysicalResourceId is the adopted resource's own physical id, the subsequent stack-level tag update leaves it UPDATE_COMPLETE, and the adopted resource is never re-created (its CreatedTimestamp is unchanged). Reference: https://docs.aws.amazon.com/AWSCloudFormation/latest/UserGuide/resource-import-existing-stack.html. The exact StatusReason wording is recorded behavior only.

Emulator: neither leg ever settles. The replayed check is killed by the harness at its ceiling — "Error: Check timed out after 180.0s" — before any assertion is reached, i.e. the `cdk import` invocation and/or the check's DescribeStackResource poll (which itself allows 180 s for the imported resource to leave its *_IN_PROGRESS/IMPORT_COMPLETE states) never observe a terminal state. The same run shows the other cdk-driven cloudformation__sqs checks (cdk-rollback-force..., cdk-rollback-orphan..., and cloudformation__sqs__ssm.cdk-deploy-no-rollback...) completing and failing on ordinary assertions, so the cdk and node tools are present and usable in this environment and the stall is the emulator's import path, not the runner.

Why: a client adopting existing infrastructure gets no answer at all — no FAILED change set to tell it a ResourcesToImport identifier was wrong, and no IMPORT_COMPLETE to tell it the adoption succeeded. Every tool that polls for a terminal state (the CDK CLI, `aws cloudformation wait`, CI pipelines) hangs rather than failing or succeeding, which is worse than a wrong answer.

Fix: the IMPORT branch of the change-set creator/executor and the stack-state machine it drives — the change set must be settled on FAILED (with the missing-resource StatusReason) when an entry resolves to nothing, and on CREATE_COMPLETE/then IMPORT_IN_PROGRESS -> IMPORT_COMPLETE for an existing resource, with the imported resource's PhysicalResourceId being the adopted resource and the stack-level tag update completing to UPDATE_COMPLETE.

Related open issue: 20261102T094631Z-cloudformation-sqs-4-net-new-causes-an-import.md records in its Cause 1 that this same check previously failed with 'expected one FAILED change set, got []' (CreateChangeSet with ChangeSetType IMPORT answering NotImplemented). It does not carry this failure's current replay key: the check no longer reaches that assertion at all, it now hangs to the harness ceiling, so the import path must be made to terminate as well as to fail correctly.

Traces: cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one.

## Evidence
Failing check (behavioral suite simparity-behavioral-aws-v1, service heading `### cloudformation__sqs`):

  [mismatch] cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one
  Error: Check timed out after 180.0s

Expected (what the check asserts, and what AWS does):
- `cdk import` with an inline resource mapping naming a queue URL that names no queue exits 1; ListChangeSets shows exactly one change set with Status FAILED; DescribeChangeSet returns (Status, StatusReason) == ("FAILED", "Resource of type 'AWS::SQS::Queue' with identifier '<the mapped queue URL>' was not found."); DescribeStacks still reports CREATE_COMPLETE and the stack's resources are unchanged ({"Handle": "CREATE_COMPLETE"}).
- `cdk import` with the mapping naming the queue created out of band exits 0; DescribeStacks reports IMPORT_COMPLETE; DescribeStackResource for logical id "Queue" settles at ResourceStatus UPDATE_COMPLETE with PhysicalResourceId equal to that queue's URL; GetQueueAttributes CreatedTimestamp is unchanged (the queue was adopted, not re-created).

Actual: the check never reaches any of those assertions — it is terminated at the 180 s harness ceiling, so no terminal change-set status and no terminal stack status is ever observed. The identifiers in the recorded run (stack name, queue name, queue URL) are per-run values; the requirement is the terminal states and the StatusReason template above.

Control: other cdk-driven checks in the same replay (cloudformation__sqs.cdk-rollback-force-..., cloudformation__sqs.cdk-rollback-orphan-..., cloudformation__sqs__ssm.cdk-deploy-no-rollback-...) run cdk to completion and fail on assertions, so cdk/node are functional here.

## Steps to Reproduce
1. CreateQueue a queue out of band (VisibilityTimeout 45) and note its URL and CreatedTimestamp.
2. Deploy a stack containing only an AWS::CloudFormation::WaitConditionHandle; confirm CREATE_COMPLETE.
3. CreateChangeSet on that stack with ChangeSetType=IMPORT and a ResourcesToImport entry of type AWS::SQS::Queue whose identifier is a queue URL that names no queue (e.g. the URL above with a different, nonexistent queue name), then poll DescribeChangeSet: it must settle FAILED with StatusReason "Resource of type 'AWS::SQS::Queue' with identifier '<that URL>' was not found." — the emulator never settles.
4. Repeat with ResourcesToImport naming the existing queue and execute the change set; poll DescribeStacks for IMPORT_COMPLETE and DescribeStackResource("Queue") for a terminal ResourceStatus — the emulator never settles. Equivalent end-to-end driver: `cdk import <stack> --resource-mapping-inline '{"Queue":{"QueueUrl":"<url>"}}'`.
5. Re-replay the check in trace_ids.
