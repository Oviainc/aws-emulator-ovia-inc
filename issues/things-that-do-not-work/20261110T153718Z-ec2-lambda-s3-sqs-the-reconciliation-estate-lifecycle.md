Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3__sqs.e2e-reconciliation-no-output-lifecycle
Issue-Key: ec2__lambda__s3__sqs.e2e-reconciliation-no-output-lifecycle|_reap_log_group#9|Timeout|return attempt
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# ec2__lambda__s3__sqs: the reconciliation estate lifecycle never completes a phase — deploy, seed, two trials, the cross-region fault derivation, cleanup and destroy are all unreached

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack / DescribeStacks (reconciliation estate lifecycle)

## Description
One net-new root cause in the ec2__lambda__s3__sqs suite. It is NOT covered by the open issues for this heading (20261105T074141Z-ec2-lambda-s3-sqs-2-net-new-createvpcendpoint.md, 20261104T044826Z-ec2-lambda-s3-sqs-createvpcendpoint-refuses-route-table-associations-and-masks-the.md), which carry other checks' replay keys for CreateVpcEndpoint refusals; this check carries none.

Cause 1 of 1: the isolated reconciliation estate never gets through a single graded phase, so none of the facts the check derives from live reads can be produced.
AWS: recorded behavior only for the end-to-end lifecycle. The estate deploys to CREATE_COMPLETE, is seeded, drives two independent reconciliation trials through it, lets the cross-region fault be derived from live reads with a fixed operation sequence, is emptied where destroy cannot empty it (the archive bucket is plain DeletionPolicy Delete and holds objects, so destroy DELETE_FAILEDs unless it is emptied first), and is then destroyed to DELETE_COMPLETE. Each phase depends on the one before, so a target that fakes any phase fails.
Emulator: the check is reported "Error: Check timed out after 60.0s" with no assertion reached. The check does not report a missing tool — it raises RequiredToolMissing(["cloudformation"]) when CloudFormation is unavailable, and that is not what happened — so the estate was being driven and simply never progressed.
Why: a client cannot deploy, exercise or tear down this estate, and none of the live reads an operator would use to attribute a cross-region fault return anything. Because no phase completes, a target that implements none of the lifecycle is indistinguishable from one that implements part of it.
Fix: the stack lifecycle and the resource providers this estate depends on (the EC2 network facts it asserts on, the Lambda/S3/SQS resources it seeds and drains, and the DELETE path that must report DELETE_FAILED on a non-empty bucket and DELETE_COMPLETE once emptied) must each reach a terminal state promptly rather than never.
Traces: ec2__lambda__s3__sqs.e2e-reconciliation-no-output-lifecycle.

## Evidence
Check ec2__lambda__s3__sqs.e2e-reconciliation-no-output-lifecycle, verdict [mismatch]:
  Error: Check timed out after 60.0s
with no assertion reached.

The awaited requirement, from the check's own docstring: "Deploy an isolated reconciliation estate, seed it, drive two independent trials through it, derive the cross-region fault from live reads with a fixed operation sequence, empty what destroy cannot, and destroy it. Each phase depends on the one before, so a target that fakes any phase fails." The check's own notes record that a VPC-attached function made `destroy` never terminate in the reference (a subnet answered DependencyViolation after 610 s of retries), so the carve asserts the fault as the estate's network facts instead, and that its `cleanup` phase is LOAD-BEARING because the archive bucket is plain DeletionPolicy Delete and holds objects by then. The check raises RequiredToolMissing(["cloudformation"]) when CloudFormation is absent; it did not, so the estate was being driven.

Expected: each phase completes in order, ending at DELETE_COMPLETE with nothing left behind. Actual: no phase completes inside the run budget, so nothing was compared. Stack names, bucket names, queue URLs and subnet/VPC ids are per-run values.

## Steps to Reproduce
1. Deploy the reconciliation estate stack (network, Lambda, archive bucket with DeletionPolicy Delete, queues) and wait for CREATE_COMPLETE.
2. Seed it and drive two independent trials through it; read the live network facts with the fixed operation sequence and derive the cross-region fault.
3. Empty the archive bucket (the `cleanup` phase), then DeleteStack and wait for DELETE_COMPLETE; confirm nothing is left behind, and that skipping the empty step yields DELETE_FAILED on that bucket.
4. The emulator completes none of these phases. Re-replay ec2__lambda__s3__sqs.e2e-reconciliation-no-output-lifecycle to confirm the fix.
