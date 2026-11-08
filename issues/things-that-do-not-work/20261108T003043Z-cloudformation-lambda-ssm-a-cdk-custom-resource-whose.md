Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__ssm.cdk-custom-resource-onevent-raise-create-failed-rolls-back-cleanly
Issue-Key: cloudformation__lambda__ssm.cdk-custom-resource-onevent-raise-create-failed-rolls-back-cleanly|run#14|AssertionError|assert status == "ROLLBACK_COMPLETE", f"expected ROLLBACK_COMPLETE, got {status!r}"
Issue-Commit: 755fa21f4b5f658e5c42cafb46f764c1d29f1673

# cloudformation__lambda__ssm: a CDK custom resource whose onEvent handler raises leaves the stack in ROLLBACK_FAILED instead of ROLLBACK_COMPLETE

> _Note: this issue was identified against the code base at commit `755fa21f4b5f658e5c42cafb46f764c1d29f1673`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStack

## Description
Cause 1 of 1: a custom resource whose onEvent handler raises on Create is not treated as never-created, so the automatic rollback fails to delete it and the stack is stuck.
AWS: when a custom resource's onEvent handler raises during Create, CloudFormation records `CREATE_FAILED` for that resource and rolls the stack back cleanly to `ROLLBACK_COMPLETE` — a resource whose create failed is not deleted again, so the rollback itself cannot fail. Smithy: cloudformation `StackStatus` distinguishes `ROLLBACK_COMPLETE` from `ROLLBACK_FAILED`, the latter reserved for a rollback that could not complete.
Emulator: the stack ends `ROLLBACK_FAILED`.
Why: `ROLLBACK_FAILED` is a terminal operator-intervention state — the stack can no longer be updated, and a normal `DeleteStack` does not clear it. A client whose custom-resource handler throws (the ordinary way a provider signals failure) is left with an unusable stack rather than a clean rollback it can fix and redeploy.
Fix: the CloudFormation rollback path must mark a resource whose CREATE failed as not-created and skip/tolerate its delete, so the rollback terminates in `ROLLBACK_COMPLETE`.
Traces: cloudformation__lambda__ssm.cdk-custom-resource-onevent-raise-create-failed-rolls-back-cleanly. The sibling checks in this service are OWNED by 20261103T061702Z-cloudformation-lambda-ssm-3-net-new-root-causes.md and 20261029T211043Z-cloudformation-lambda-ssm-cloudformation-a-failed-ssm-dynamic-reference-produces-no.md; neither carries this check's replay key.

## Evidence
Expected: `ROLLBACK_COMPLETE` after the custom resource's onEvent Create raises.
Actual: `AssertionError: expected ROLLBACK_COMPLETE, got 'ROLLBACK_FAILED'`.

## Steps to Reproduce
1. `cdk bootstrap`, then deploy a CDK stack with a custom resource whose onEvent Lambda raises on the Create request.
2. Wait for the stack to reach a terminal status.
3. `DescribeStacks`: StackStatus must be `ROLLBACK_COMPLETE`; the emulator reports `ROLLBACK_FAILED`.
