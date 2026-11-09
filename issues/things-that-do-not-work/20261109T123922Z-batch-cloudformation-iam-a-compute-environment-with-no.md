Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__cloudformation__iam.compute-environment-missing-service-role-rejected batch__cloudformation__iam.compute-environment-tag-change-forces-replacement
Issue-Key: batch__cloudformation__iam.compute-environment-missing-service-role-rejected|run#22|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: batch__cloudformation__iam.compute-environment-tag-change-forces-replacement|run#13|AssertionError|assert before["tags"] == {"sim2real-env": "v1"}, before["tags"]
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# batch__cloudformation__iam: a compute environment with no ServiceRole is accepted, and a tag-only change does not force the replacement it must

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two independent causes behind the 2 failing batch__cloudformation__iam checks.

Cause 1 of 2: AWS::Batch::ComputeEnvironment with no ServiceRole is provisioned instead of refused.
AWS: recorded behavior only — a managed compute environment declared without a ServiceRole fails to create; the stack settles ROLLBACK_COMPLETE with AWS Batch's own refusal as the resource failure reason.
Emulator: the stack settles 'CREATE_COMPLETE' — the missing ServiceRole is never checked.
Why: a client gets a compute environment AWS would never have created, with no service role to assume, so every later scaling or job-placement call behaves differently from AWS; the authoring error is never reported.
Fix: the AWS::Batch::ComputeEnvironment resource handler's create validation (ServiceRole requirement for the declared Type), and the stack rollback that failure must drive.
Traces: batch__cloudformation__iam.compute-environment-missing-service-role-rejected

Cause 2 of 2: a tag change on AWS::Batch::ComputeEnvironment does not produce the replacement AWS performs, and the resulting tag set is wrong.
AWS: recorded behavior only — Tags on AWS::Batch::ComputeEnvironment is a create-only property, so changing it forces a replacement; the replacing resource carries the stack's three system tags (aws:cloudformation:stack-name, aws:cloudformation:stack-id, aws:cloudformation:logical-id) plus the template's own tags for the NEW tag value.
Emulator: the tag set read back after the update is {'aws:cloudformation:stack-name': ..., 'aws:cloudformation:stack-id': ..., 'aws:cloudformation:logical-id': 'CE', '<template-tag-key>': '<the pre-update value>'} — the environment was updated in place and still carries the old template tag value instead of being replaced.
Why: a client that relies on a tag change forcing a replacement (to re-create a compute environment under a new identity) silently keeps the original resource, and anything that selects compute environments by tag selects the wrong one.
Fix: the create-only/replacement classification of the Tags property in the AWS::Batch::ComputeEnvironment resource handler, and its tag write-through on update.
Traces: batch__cloudformation__iam.compute-environment-tag-change-forces-replacement

## Evidence
batch__cloudformation__iam.compute-environment-missing-service-role-rejected — AssertionError: stack '<name>' without a ServiceRole ended in 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE

batch__cloudformation__iam.compute-environment-tag-change-forces-replacement — AssertionError: {'aws:cloudformation:stack-name': '<stack>', 'aws:cloudformation:stack-id': 'arn:aws:cloudformation:<region>:<account>:stack/<stack>/<uuid>', 'aws:cloudformation:logical-id': 'CE', '<tag-key>': '<old-value>'}  (expected the replacement's tag set, carrying the NEW template tag value)

## Steps to Reproduce
1. Cause 1: CreateStack with a managed AWS::Batch::ComputeEnvironment that declares no ServiceRole; expect ROLLBACK_COMPLETE, observe CREATE_COMPLETE.
2. Cause 2: deploy an AWS::Batch::ComputeEnvironment with a Tags entry, then UpdateStack changing only that tag's value; expect the resource to be replaced and to carry the new tag value, observe an in-place update that keeps the old value.
3. Re-replay both checks in trace_ids.
