Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__cloudformation.deploy-consumable-resource-lifecycle batch__cloudformation.deploy-consumable-resource-rejects-negative-quantity batch__cloudformation.deploy-job-definition-lifecycle batch__cloudformation.deploy-scheduling-policy-lifecycle batch__cloudformation.deploy-scheduling-policy-rejects-out-of-range-compute-reservation batch__cloudformation.deploy-service-environment-and-quota-share-lifecycle
Issue-Key: batch__cloudformation.deploy-consumable-resource-lifecycle|run#9|AssertionError|assert out["ResourceArnAtt"] == arn, f"GetAtt {out['ResourceArnAtt']!r} != Ref {arn!r}"
Issue-Key: batch__cloudformation.deploy-consumable-resource-rejects-negative-quantity|run#10|AssertionError|assert got == [(good_arn, "REPLENISHABLE", 10)], f"control resource reads {got!r}"
Issue-Key: batch__cloudformation.deploy-job-definition-lifecycle|run#10|AssertionError|assert (jd_ref, out["JobDefArnAtt"]) == (want_arn, want_arn), (
Issue-Key: batch__cloudformation.deploy-scheduling-policy-lifecycle|run#9|AssertionError|assert out["PolicyArnAtt"] == arn and _listed(batch, name) == [arn], (
Issue-Key: batch__cloudformation.deploy-scheduling-policy-rejects-out-of-range-compute-reservation|run#9|AssertionError|assert _listed(batch, good_name) == [good_arn], f"control policy {good_arn!r} not listed"
Issue-Key: batch__cloudformation.deploy-service-environment-and-quota-share-lifecycle|run#10|AssertionError|assert (out["SeArnAtt"], out["QsArnAtt"]) == (se_arn, qs_arn), f"GetAtt ARNs {out!r}"
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# batch__cloudformation: 2 net-new causes — CloudFormation-created Batch resources are never registered in Batch's own state, and Ref/GetAtt return the wrong physical identifiers

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two independent causes behind the 6 failing batch__cloudformation checks. The stacks reach a terminal status, so these are not registry gaps — the Batch resource handlers mint identifiers and never write through to the Batch service.

Cause 1 of 2: a Batch resource created by a stack is not visible through the Batch API that owns it.
AWS: recorded behavior only — after a stack declaring AWS::Batch::ConsumableResource, ::SchedulingPolicy, ::JobDefinition, ::ServiceEnvironment or ::JobQueue reaches CREATE_COMPLETE, the matching Batch List/Describe operation returns the resource, so a client can read back what the template created.
Emulator: the Batch read returns an empty list for the resource the stack just created — the checks report `control resource reads []`, `control policy '<name>' not listed` and `Ref '<name>', GetAtt '<arn>', listed []`.
Why: a client cannot read, update or operate on anything Batch that a template created. Any workflow that deploys a Batch estate by template and then submits jobs against it fails, because the queues, job definitions, scheduling policies and consumable resources do not exist from the Batch API's point of view — and a negative leg's positive control cannot even be established.
Fix: the AWS::Batch::* resource handlers' create path — each must call the batch crate's Create* operation (or write the same state) so the resource exists in Batch state, rather than only recording a CloudFormation resource row.
Traces: batch__cloudformation.deploy-consumable-resource-rejects-negative-quantity, batch__cloudformation.deploy-scheduling-policy-lifecycle, batch__cloudformation.deploy-scheduling-policy-rejects-out-of-range-compute-reservation

Cause 2 of 2: Ref and GetAtt on AWS::Batch::* resources return identifiers that are not the ones AWS returns.
AWS: recorded behavior only — for AWS::Batch::ConsumableResource, Ref returns the resource's ARN (the same value as the Arn attribute); for AWS::Batch::JobDefinition the ARN is revision-qualified, ending `:<revision>` (`:1` for a freshly created definition); for AWS::Batch::ServiceEnvironment, ::JobQueue and ::QuotaShare each Ref/GetAtt pairing follows the type's documented return values rather than being mixed per resource.
Emulator: Ref returns the bare resource NAME while GetAtt returns the ARN (`GetAtt 'arn:aws:batch:<region>:<account>:consumable-resource/<name>' != Ref '<name>'`); the JobDefinition ARN omits the `:<revision>` suffix (`'arn:aws:batch:<region>:<account>:job-definition/<name>'`, expected `'arn:aws:batch:<region>:<account>:job-definition/<name>:1'`); and in the service-environment stack the Ref/GetAtt values are inconsistent across resources — SpRef and QsRef are bare names while JqRef is an ARN.
Why: every template that wires one Batch resource into another with Ref (a job queue referencing its scheduling policy, a job submission referencing the job definition) passes a value the receiving API cannot resolve, and a client that stores the JobDefinition ARN cannot address a specific revision — the revision is the part that pins the definition.
Fix: the Ref/GetAtt return-value tables of the AWS::Batch::* resource handlers, and the ARN construction for AWS::Batch::JobDefinition (it must append the revision).
Traces: batch__cloudformation.deploy-consumable-resource-lifecycle, batch__cloudformation.deploy-job-definition-lifecycle, batch__cloudformation.deploy-service-environment-and-quota-share-lifecycle

## Evidence
Cause 1:
  batch__cloudformation.deploy-consumable-resource-rejects-negative-quantity — AssertionError: control resource reads []
  batch__cloudformation.deploy-scheduling-policy-rejects-out-of-range-compute-reservation — AssertionError: control policy '<name>' not listed
  batch__cloudformation.deploy-scheduling-policy-lifecycle — AssertionError: Ref '<name>', GetAtt 'arn:aws:batch:<region>:<account>:scheduling-policy/<name>', listed []
  Expected in each: the Batch List/Describe operation returns the resource the stack created.

Cause 2:
  batch__cloudformation.deploy-consumable-resource-lifecycle — AssertionError: GetAtt 'arn:aws:batch:<region>:<account>:consumable-resource/<name>' != Ref '<name>'  (expected Ref == the ARN)
  batch__cloudformation.deploy-job-definition-lifecycle — AssertionError: Ref '<name>', GetAtt 'arn:aws:batch:<region>:<account>:job-definition/<name>', expected 'arn:aws:batch:<region>:<account>:job-definition/<name>:1'
  batch__cloudformation.deploy-service-environment-and-quota-share-lifecycle — AssertionError: GetAtt ARNs {'SpRef': '<name>', 'SeRef': 'arn:...:service-environment/<name>', 'SeArnAtt': 'arn:...:service-environment/<name>', 'QsRef': '<name>', 'QsArnAtt': 'arn:...:quota-share/<name>', 'JqRef': 'arn:...:job-queue/<name>'}

## Steps to Reproduce
1. Cause 1: CreateStack with a template declaring AWS::Batch::SchedulingPolicy (or ::ConsumableResource); wait for CREATE_COMPLETE; call the matching Batch List/Describe operation and observe an empty list.
2. Cause 2: in the same stack, export Ref and the Arn attribute of each AWS::Batch::* resource as stack Outputs and compare them to the values AWS returns — Ref of a ConsumableResource must equal its ARN, and a JobDefinition ARN must end `:1`.
3. Re-replay the 6 checks in trace_ids.
