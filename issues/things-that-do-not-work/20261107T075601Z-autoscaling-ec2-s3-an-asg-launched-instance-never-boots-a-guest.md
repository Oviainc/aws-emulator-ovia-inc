Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__s3.asg-instance-role-scoped-to-one-prefix-is-denied-the-other-write
Issue-Key: autoscaling__ec2__s3.asg-instance-role-scoped-to-one-prefix-is-denied-the-other-write|_poll#10|Timeout|raise AssertionError(
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# An ASG-launched instance never boots a guest, so instance-role S3 scenarios never converge

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
One net-new root cause.

Cause 1 of 1: instances launched by an Auto Scaling group never present a running guest, so nothing the guest is supposed to do ever happens.
AWS: an ASG instance launched from a launch template with an instance profile and UserData boots, runs the UserData, and its instance-role credentials are used to write to S3 — a prefix-scoped role then succeeds on its own prefix and is denied AccessDenied on the other (recorded behavior).
Emulator: the instance is reported but no guest runs: the scenario's wait for the S3 write/denial never completes and the check ends "Check timed out after 60.0s".
Why: no IAM-on-the-instance scenario (instance profile scoping, credential delivery through IMDS) can be exercised; a client cannot distinguish "the policy allows it" from "nothing ran".
Fix: the EC2 instance runtime the ASG launch path produces (guest boot + UserData execution + instance-profile credential delivery), i.e. the same instance materialisation EC2's RunInstances path owns.
Traces: autoscaling__ec2__s3.asg-instance-role-scoped-to-one-prefix-is-denied-the-other-write.

## Evidence
From the behavioral replay:
- asg-instance-role-scoped-to-one-prefix-is-denied-the-other-write [mismatch]: Error: Check timed out after 60.0s.
(The sibling check asg-launched-instance-boots-and-writes-its-own-identity failed with TimeoutError "... the run budget ended first; not graded" and is therefore not filed as part of this cause.)

## Steps to Reproduce
Replay the behavioral suite and re-run autoscaling__ec2__s3.asg-instance-role-scoped-to-one-prefix-is-denied-the-other-write: create a bucket, a role scoped to one prefix, a launch template with that instance profile and UserData that writes an object, an ASG with desired capacity 1, then wait for the allowed write to appear and the other prefix write to be denied.
