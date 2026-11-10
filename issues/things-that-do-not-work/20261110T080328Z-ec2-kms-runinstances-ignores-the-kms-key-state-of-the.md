Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__kms.asglf-run-instances-under-disabled-key-blocked ec2__kms.asglf-run-instances-under-pending-deletion-or-absent-key-blocked
Issue-Key: ec2__kms.asglf-run-instances-under-disabled-key-blocked|run#12|AssertionError|assert settled == ("terminated", _REASON), (
Issue-Key: ec2__kms.asglf-run-instances-under-pending-deletion-or-absent-key-blocked|run#13|AssertionError|assert results == {"pending-deletion": ("terminated", _REASON),
Issue-Commit: b4658095b9c4860859c50d7c6684da8fd2f8280e

# RunInstances ignores the KMS key state of the root volume: a launch under a disabled, pending-deletion or absent CMK stays running instead of terminating with Client.InvalidKMSKey.InvalidState

> _Note: this issue was identified against the code base at commit `b4658095b9c4860859c50d7c6684da8fd2f8280e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
One net-new root cause, exercised through three key states by two checks.

AWS: a launch whose block-device mapping encrypts the root volume with a customer managed key is ACCEPTED by RunInstances (HTTP 200, the instance appears pending) and then settles asynchronously: because the EBS volume cannot be created, the instance transitions to "terminated" and DescribeInstances reports StateReason/StateTransitionReason code "Client.InvalidKMSKey.InvalidState" for the instance. The same outcome applies to every key the EBS attachment cannot use — a key whose KeyState is Disabled, a key whose KeyState is PendingDeletion (after kms:ScheduleKeyDeletion), and a well-formed key ARN that does not exist (kms:DescribeKey answers NotFoundException "Key '<key-arn>' does not exist"). Recorded behavior only for the asynchronous settle; the terminal state "terminated" and the reason code "Client.InvalidKMSKey.InvalidState" are the requirement (the reason code is fixed text; the key ARN is the value from the request).

Emulator: for all three key states the instance settles "running" with no state reason at all — the launch is accepted and the guest simply runs, as if the key were enabled and present. The KMS key state is never consulted when the encrypted root volume is created.

Why: a client's entire safety story for key revocation rests on this. Disabling or scheduling deletion of a CMK is the documented way to stop new workloads from using data encrypted under it; here the launch succeeds anyway, so a revoked key appears to still permit new encrypted volumes. Clients that poll for "terminated" with Client.InvalidKMSKey.InvalidState (to alert, or to fail a deployment) instead see a healthy running instance, and a dangling key ARN that exists nowhere is silently accepted.

Fix: the block-device/EBS encryption path of services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs together with the volume-creation state it drives — resolve the KmsKeyId against KMS, and when the key is Disabled, PendingDeletion or absent, move the instance to terminated and record StateReason code Client.InvalidKMSKey.InvalidState instead of leaving it running.

Related: the open issue 20261107T075601Z-ec2-kms-runinstances-with-a-launchtemplate-answers-501-notimplemented-so.md reported these same KMS key-state refusals as unreachable because the launch-template branch of RunInstances was refused. The launch now succeeds, so the refusal is reachable — and is not applied. This is the net-new behaviour behind that earlier blocker.

Traces: ec2__kms.asglf-run-instances-under-disabled-key-blocked (Disabled), ec2__kms.asglf-run-instances-under-pending-deletion-or-absent-key-blocked (PendingDeletion and absent ARN).

## Evidence
ec2__kms.asglf-run-instances-under-disabled-key-blocked:
AssertionError: an instance under a disabled key settled ('running', None), expected ('terminated', 'Client.InvalidKMSKey.InvalidState')

ec2__kms.asglf-run-instances-under-pending-deletion-or-absent-key-blocked:
AssertionError: launches settled {'pending-deletion': ('running', None), 'absent': ('running', None)}
Expected {'pending-deletion': ('terminated', 'Client.InvalidKMSKey.InvalidState'), 'absent': ('terminated', 'Client.InvalidKMSKey.InvalidState')}.
The check's precondition reads confirmed the key states first: kms:DescribeKey reported KeyState 'PendingDeletion' for the scheduled key and NotFoundException "Key '<arn>' does not exist" for the dangling ARN.

## Steps to Reproduce
1. kms:CreateKey (key A) and kms:DisableKey(A). kms:CreateKey (key B) and kms:ScheduleKeyDeletion(B, PendingWindowInDays=7). Build a third, well-formed key ARN in the same account/region whose key id does not exist.
2. For each of the three key references: ec2:CreateLaunchTemplate whose LaunchTemplateData.BlockDeviceMappings[0].Ebs = {Encrypted: true, KmsKeyId: <reference>}, then ec2:RunInstances from it into a subnet.
3. Poll ec2:DescribeInstances until the instance settles. Expected State.Name == 'terminated' with StateReason.Code == 'Client.InvalidKMSKey.InvalidState'; the emulator settles 'running' with no state reason.
4. Re-replay both check ids in trace_ids.
