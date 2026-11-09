Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack
Issue-Key: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack|run#13|AssertionError|assert final_status == "ROLLBACK_COMPLETE", (
Issue-Commit: 755fa21f4b5f658e5c42cafb46f764c1d29f1673

# cloudformation__dynamodb__lambda: a custom resource whose create times out leaves the stack in ROLLBACK_FAILED instead of ROLLBACK_COMPLETE

> _Note: this issue was identified against the code base at commit `755fa21f4b5f658e5c42cafb46f764c1d29f1673`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStack

## Description
Cause 1 of 1: when a custom resource's create never completes and CloudFormation gives up waiting, the automatic rollback cannot delete that resource and the stack ends in `ROLLBACK_FAILED`.
AWS: a `custom_resources.Provider` custom resource whose isComplete handler never reports done makes CloudFormation record `CREATE_FAILED` on the custom resource with the reason `<physical-id> was still CREATE_IN_PROGRESS when the wait for it to be created ran out` (fixed text; the physical id comes from the stack), then roll the whole stack back to `ROLLBACK_COMPLETE` — a resource whose create failed is deleted (or treated as absent) by the rollback, never left to fail its delete. Smithy: cloudformation `StackStatus` includes both `ROLLBACK_COMPLETE` and `ROLLBACK_FAILED`; `ROLLBACK_FAILED` is reserved for a rollback that could not complete and requires operator intervention.
Emulator: the stack ends `ROLLBACK_FAILED` with `The following resource(s) failed to delete: [Waited].` — the timed-out custom resource is handed to the rollback as a live resource and its delete fails, so the stack is left stuck instead of cleanly rolled back.
Why: `ROLLBACK_FAILED` is a terminal, operator-intervention state: the stack cannot be updated or deleted normally, and a client's deploy pipeline that retries after an automatic rollback is blocked. A timed-out custom resource is an ordinary failure in AWS and must not poison the stack.
Fix: the CloudFormation stack-operation state machine's rollback path must treat a resource whose CREATE never completed as not-created (skip or tolerate its delete) so the rollback reaches `ROLLBACK_COMPLETE`.
Traces: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack. The sibling check cdk-custom-resource-provider-waits-for-iscomplete is OWNED by 20261105T204906Z-cloudformation-dynamodb-lambda-a-custom-resources-provider-custom.md; that issue does not carry this check's replay key.

## Evidence
Expected: `ROLLBACK_COMPLETE` — a timed out custom-resource create must roll the stack back, not leave it CREATE_FAILED or partially applied.
Actual: `AssertionError: stack 'sim2real-cdkwaiter-<id>' ended 'ROLLBACK_FAILED', expected ROLLBACK_COMPLETE`. The stack events show `CREATE_FAILED | AWS::CloudFormation::CustomResource | Waited/Default ... was still CREATE_IN_PROGRESS when the wait for it to be created ran out` followed by `ROLLBACK_FAILED ... The following resource(s) failed to delete: [Waited].`

## Steps to Reproduce
1. `cdk bootstrap`, then deploy a CDK stack containing a `custom_resources.Provider` whose isComplete handler always answers IsComplete=false.
2. Wait for CloudFormation to stop waiting for the custom resource's create.
3. `DescribeStacks` the stack: its StackStatus must be `ROLLBACK_COMPLETE`; the emulator reports `ROLLBACK_FAILED` with `The following resource(s) failed to delete: [<logical-id>].`
