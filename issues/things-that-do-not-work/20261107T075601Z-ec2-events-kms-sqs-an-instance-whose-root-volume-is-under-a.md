Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__events__kms__sqs.xsvc-ec2-2-run-instances-under-disabled-key-terminates-with-client-kms-reason-rejected
Issue-Key: ec2__events__kms__sqs.xsvc-ec2-2-run-instances-under-disabled-key-terminates-with-client-kms-reason-rejected|run#20|AssertionError|assert fate["State"]["Name"] == "terminated", (
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# An instance whose root volume is under a disabled KMS key reaches running instead of terminating

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
One net-new root cause.

Cause 1 of 1: a launch whose root volume is encrypted with a disabled KMS key is allowed to run.
AWS: the launch is accepted, but the root volume cannot be created — the instance moves pending -> shutting-down -> terminated and its StateReason carries the client KMS reason (Client.InternalError / the KMS key-state cause), with the matching state-change notification; it never reaches running (recorded behavior; kms DisabledException on the Encrypt/CreateGrant the volume needs).
Emulator: the instance settles as {'Code': 16, 'Name': 'running'} with no state reason, i.e. the disabled key is never consulted.
Why: a key-state failure that AWS surfaces as a terminated instance is invisible; a caller validating encryption-at-rest key state sees a healthy fleet, and the documented KMS-caused termination path cannot be tested.
Fix: the RunInstances/EBS volume-creation path's KMS key-state check and the instance termination + StateReason it must produce.
Traces: ec2__events__kms__sqs.xsvc-ec2-2-run-instances-under-disabled-key-terminates-with-client-kms-reason-rejected.

## Evidence
From the behavioral replay:
- xsvc-ec2-2-run-instances-under-disabled-key-terminates-with-client-kms-reason-rejected [mismatch]: AssertionError: an instance whose root volume is under a disabled key must never run; settled as {'Code': 16, 'Name': 'running'} None.

## Steps to Reproduce
Replay the behavioral suite and re-run ec2__events__kms__sqs.xsvc-ec2-2-run-instances-under-disabled-key-terminates-with-client-kms-reason-rejected: create a customer-managed KMS key, disable it, launch an instance whose root block-device mapping is encrypted with that key, then poll the instance state and StateReason.
