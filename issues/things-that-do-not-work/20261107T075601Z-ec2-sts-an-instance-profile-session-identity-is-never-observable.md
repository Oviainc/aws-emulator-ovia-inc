Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__sts.instance-profile-session-userid-is-the-role-id-and-instance-id-composite
Issue-Key: ec2__sts.instance-profile-session-userid-is-the-role-id-and-instance-id-composite|_poll#11|Timeout|time.sleep(interval_s)
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# An instance-profile session identity is never observable: the STS caller-identity derivation never converges

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCallerIdentity

## Description
One net-new root cause.

Cause 1 of 1: an instance using its instance profile never produces an STS session whose identity can be read, so the composite UserId assertion never completes.
AWS: credentials served to an instance through its instance profile belong to an assumed-role session whose GetCallerIdentity UserId is the composite "<role id>:<instance id>" and whose Arn is arn:aws:sts::<account>:assumed-role/<role name>/<instance id> (recorded behavior; sts GetCallerIdentityResponse UserId/Arn/Account).
Emulator: the scenario never reaches a caller identity — the check ends "Check timed out after 60.0s" because the instance never serves instance-profile credentials.
Why: the documented way to prove which instance a call came from is unavailable, so no instance-scoped authorization can be verified.
Fix: the instance-profile credential path (IMDS credentials from the instance runtime) and the STS session identity it must mint.
Traces: ec2__sts.instance-profile-session-userid-is-the-role-id-and-instance-id-composite.

## Evidence
From the behavioral replay:
- ec2__sts.instance-profile-session-userid-is-the-role-id-and-instance-id-composite [mismatch]: Error: Check timed out after 60.0s.

## Steps to Reproduce
Replay the behavioral suite and re-run ec2__sts.instance-profile-session-userid-is-the-role-id-and-instance-id-composite: create a role and instance profile, launch an instance with it, and have the instance's credentials call sts:GetCallerIdentity, then compare UserId/Arn to the role id + instance id composite.
