Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-t12-iam-profile-replace-and-stop-echo
Issue-Step: teardown#25
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-t12-iam-profile-replace-and-stop-echo
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: iam|DeleteRole|2xx->4xx|status
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# IAM DeleteRole answers 409 on a role whose instance profile was detached

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRole

## Description
One cause of 1, reached through an EC2-led fixture but belonging to IAM (the divergence reports service 'iam', and iam is bootstrapped).

Cause 1 of 1: DeleteRole refuses a role that no longer has anything attached.
AWS: HTTP 200 - once the role has been removed from its instance profile (and its policies detached), DeleteRole succeeds. The fixture's teardown performs exactly that sequence before deleting.
Emulator: HTTP 409 - IAM still believes the role is in use, i.e. the preceding RemoveRoleFromInstanceProfile / detach did not clear the role's in-use linkage in IAM state. The response body was not asserted beyond the status, so the error code is not part of this finding; the status is.
Why: any instance-profile lifecycle that creates a role, attaches it, swaps it and then tears down cannot complete - the role is undeletable and leaks, and a teardown script that treats 409 as fatal aborts. Because the role survives, re-running the same scenario hits a name conflict on create.
Fix: the in-use/attachment bookkeeping IAM's DeleteRole consults, and the RemoveRoleFromInstanceProfile path that should clear it (IAM role state, not the DeleteRole validation alone).
Traces: ec2-t12-iam-profile-replace-and-stop-echo#teardown#25.

## Evidence
'status: expected 200, got 409' for service=iam|DeleteRole in ec2-t12-iam-profile-replace-and-stop-echo#teardown#25; divergence_key service=iam|DeleteRole|status=200->409|keys=status. No stub_path and no owning open issue (stub_path '', owned_by []).

## Steps to Reproduce
Replay ec2-t12-iam-profile-replace-and-stop-echo and let it reach teardown#25. Minimal repro: iam:CreateRole, iam:CreateInstanceProfile, iam:AddRoleToInstanceProfile, then iam:RemoveRoleFromInstanceProfile, iam:DeleteInstanceProfile, and finally iam:DeleteRole on that role - the delete answers 409 instead of 200.
