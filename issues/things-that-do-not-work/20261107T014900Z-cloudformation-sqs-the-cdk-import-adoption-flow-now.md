Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one
Issue-Key: cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one|_poll_until#30|Timeout|return last
Issue-Commit: 486a0852ab6c107011e8dec7ea6574940ef62e53

# cloudformation__sqs: the `cdk import` adoption flow now reaches no verdict at all — the IMPORT change set neither fails for a missing queue nor completes for an existing one (1 cause)

> _Note: this issue was identified against the code base at commit `486a0852ab6c107011e8dec7ea6574940ef62e53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
One net-new root cause. The check is not marked ALREADY FILED or OWNED in the replay report. The open issue 20261102T094631Z-cloudformation-sqs-4-net-new-causes-an-import.md lists this check id and describes the import gap as an assertion failure ('expected one FAILED change set, got []'); in this replay the check no longer reaches that assertion at all — it hangs to the harness cap — so the observed signature is new and that issue's replay key does not cover it. A fixer should close both together.

Cause 1 of 1: CloudFormation resource import produces neither of its two outcomes, and the flow hangs.
AWS: Smithy (cloudformation CreateChangeSet -> ChangeSetType, ResourcesToImport; DescribeChangeSet -> Status/StatusReason; DescribeStacks -> StackStatus) models the surface; the import semantics are recorded behavior only. `cdk import` of a resource mapping naming an object that does not exist leaves exactly one change set in status FAILED whose StatusReason is "Resource of type 'AWS::SQS::Queue' with identifier '<queue URL>' was not found." — fixed text around the resource type and the identifier taken from the resource mapping — the CLI exits 1 and the stack is untouched (CREATE_COMPLETE with only its pre-existing resources). `cdk import` of a mapping naming an existing queue leaves the stack IMPORT_COMPLETE, then applies the stack-level tags as its own update so the imported resource settles at UPDATE_COMPLETE with PhysicalResourceId equal to that queue's URL, and the queue's CreatedTimestamp is unchanged (it was adopted, not re-created).
Emulator: neither leg produces an outcome — the check is abandoned with 'Error: Check timed out after 60.0s', so no FAILED change set, no IMPORT_COMPLETE stack and no adopted resource is ever observable.
Why: a client adopting existing infrastructure into CloudFormation gets no answer at all: a typo in the resource mapping is indistinguishable from a successful import, and an import that should adopt a live queue can neither be confirmed nor shown to have avoided re-creating it.
Fix: the IMPORT path of the change-set creator and executor (per-entry existence resolution producing the FAILED change set with its reason, and the import executor that reaches IMPORT_COMPLETE and the subsequent tag update without recreating the physical resource).
Traces: cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one.

## Evidence
Expected (missing-queue leg): one change set with Status FAILED and StatusReason "Resource of type 'AWS::SQS::Queue' with identifier '<queue URL>' was not found.", CLI exit 1, stack still CREATE_COMPLETE. Expected (existing-queue leg): CLI exit 0, stack IMPORT_COMPLETE, the Queue resource at UPDATE_COMPLETE with PhysicalResourceId = the queue URL and an unchanged CreatedTimestamp.
Actual: 'Error: Check timed out after 60.0s' — the check reaches no verdict; nothing about the import is observable.

## Steps to Reproduce
Deploy a stack without the queue, create a queue outside CloudFormation, then run `cdk import <stack> --resource-mapping-inline '{"Queue": {"QueueUrl": "<url of a queue that does not exist>"}}'` and inspect ListChangeSets/DescribeChangeSet and DescribeStacks; then repeat with the existing queue's URL and read DescribeStackResource for the Queue logical id plus the queue's CreatedTimestamp. Replay the check id in trace_ids.
