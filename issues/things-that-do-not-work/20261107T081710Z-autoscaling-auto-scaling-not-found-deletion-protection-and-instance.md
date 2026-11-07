Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.delete-nonexistent-group-fails autoscaling.create-asg-missing-launch-config-rejected autoscaling.launch-config-create-delete-lifecycle autoscaling.deep-autoscaling-2-asg-enforce-prevent-all-deletion-rejects-plain-and-force-delete autoscaling.deep-autoscaling-2-asg-enforce-prevent-force-deletion-rejects-force-delete-of-live-group autoscaling.deep-autoscaling-2-asg-enforce-prevent-force-deletion-accepts-plain-delete-of-empty-group autoscaling.cancel-instance-refresh-not-found-fails autoscaling.instance-refresh-health-spread-over-100-rejected autoscaling.instance-refresh-max-healthy-without-min-rejected autoscaling.instance-refresh-unsorted-checkpoints-rejected autoscaling.op-contract-describeInstanceRefreshes autoscaling.rollback-instance-refresh-irreversible-fails autoscaling.rollback-instance-refresh-no-active-fails autoscaling.rollback-without-active-refresh-fails autoscaling.create-launch-template-conflicting-name-rejected
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.delete-nonexistent-group-fails|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: autoscaling.create-asg-missing-launch-config-rejected|run#24|AssertionError|assert dead_message == "AutoScalingGroup name not found - null", f"expected the exact refusal message, got {dead_message!r}"
Issue-Key: autoscaling.launch-config-create-delete-lifecycle|run#28|AssertionError|assert message2 == expected, f"expected message {expected!r}, got {message2!r}"
Issue-Key: autoscaling.deep-autoscaling-2-asg-enforce-prevent-all-deletion-rejects-plain-and-force-delete|run#13|AssertionError|assert message == _PREVENT_ALL_MESSAGE, f"plain delete under prevent-all-deletion said {message!r}"
Issue-Key: autoscaling.deep-autoscaling-2-asg-enforce-prevent-force-deletion-rejects-force-delete-of-live-group|run#13|AssertionError|assert message == _PREVENT_FORCE_MESSAGE, f"ForceDelete under prevent-force-deletion said {message!r}"
Issue-Key: autoscaling.deep-autoscaling-2-asg-enforce-prevent-force-deletion-accepts-plain-delete-of-empty-group|run#11|AssertionError|assert message == _PREVENT_FORCE_MESSAGE, f"ForceDelete under prevent-force-deletion said {message!r}"
Issue-Key: autoscaling.cancel-instance-refresh-not-found-fails|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: autoscaling.instance-refresh-health-spread-over-100-rejected|run#6|AssertionError|assert msg == expected, f"the rejection does not state the cross-field rule: {msg!r}"
Issue-Key: autoscaling.instance-refresh-max-healthy-without-min-rejected|run#6|AssertionError|assert msg == expected, (
Issue-Key: autoscaling.instance-refresh-unsorted-checkpoints-rejected|run#3|AssertionError|assert msg == "Checkpoint percentages must be sorted in ascending order with no duplicates.", (
Issue-Key: autoscaling.op-contract-describeInstanceRefreshes|run#16|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: autoscaling.rollback-instance-refresh-irreversible-fails|run#16|AssertionError|assert code == "IrreversibleInstanceRefresh", (
Issue-Key: autoscaling.rollback-instance-refresh-no-active-fails|run#4|AssertionError|assert code == "IrreversibleInstanceRefresh", f"expected IrreversibleInstanceRefresh, got {code}"
Issue-Key: autoscaling.rollback-without-active-refresh-fails|run#10|AssertionError|assert code == "IrreversibleInstanceRefresh", f"expected IrreversibleInstanceRefresh, got {code}"
Issue-Key: autoscaling.create-launch-template-conflicting-name-rejected|run#9|AssertionError|assert message == "Launch template name already in use.", (
Issue-Commit: 38b869ac761e6c1ceaf9cdd20433d5027a2449ee

# Auto Scaling not-found, deletion-protection and instance-refresh refusals diverge: 9 net-new causes (wrong sentences, and RollbackInstanceRefresh answers NotImplemented)

> _Note: this issue was identified against the code base at commit `38b869ac761e6c1ceaf9cdd20433d5027a2449ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAutoScalingGroup

## Description
NINE net-new root causes in Auto Scaling's (and one EC2) refusal texts plus one unimplemented operation. Each must be closed separately. Related open issues: 20261102T114505Z-autoscaling-auto-scaling-group-scoped-operations-refuse-an-existing-group.md (an EXISTING group wrongly refused — the opposite direction), 20261030T080732Z-autoscaling-cancelinstancerefresh-group-not-found-refusal-uses-the-describe-wording.md (CancelInstanceRefresh GROUP-not-found, not the no-active-refresh sentence below), and 20261029T211352Z-autoscaling-auto-scaling-client-error-refusals-diverge-emulator-authored.md; none carries the check ids below.

Cause 1 of 9: the AutoScalingGroup-not-found sentence is emulator-authored and does not vary by operation family as AWS's does.
AWS: HTTP 400 / ValidationError. For DeleteAutoScalingGroup the message is "AutoScalingGroup name not found - AutoScalingGroup '<name>' not found" (the name is single-quoted, variable part = the requested name). For the group-scoped mutation path exercised after a refused create, AWS answers 'AutoScalingGroup name not found - null' (fixed text, the trailing token is literally 'null', no name).
Emulator: one sentence for everything — 'AutoScalingGroup name not found - AutoScalingGroup: <name> not found' (colon instead of the quoted form, name always interpolated).
Why: callers match the sentence to distinguish 'this group never existed' from the other ValidationError refusals; the emulator's single variant matches neither recorded form.
Fix: the group-lookup failure message(s) in the Auto Scaling handlers' shared not-found path.
Traces: autoscaling.delete-nonexistent-group-fails, autoscaling.create-asg-missing-launch-config-rejected.

Cause 2 of 9: the launch-configuration-not-found sentence is emulator-authored.
AWS: HTTP 400 / ValidationError with 'Launch configuration name not found - A launch configuration with the name: <name> does not exist' (fixed text around the requested name).
Emulator: 'Launch configuration name not found - Launch configuration <name> not found'.
Why: same as cause 1 — the prefix matches so a prefix test passes, but any exact comparison or regex on AWS's sentence fails.
Fix: the launch-configuration lookup failure message used by CreateAutoScalingGroup/UpdateAutoScalingGroup.
Traces: autoscaling.launch-config-create-delete-lifecycle.

Cause 3 of 9: deletion-protection refusals use an emulator-authored sentence.
AWS: HTTP 400 / ValidationError with 'You cannot delete this group because its deletion protection is set to prevent-all-deletion. If you are sure that you want to delete this group, remove deletion protection for the group and try again.' and, for the force variant, the same sentence with 'prevent-force-deletion'. Fixed text; the only variable part is the protection value itself. The group must survive.
Emulator: 'Cannot delete AutoScalingGroup <name> because its DeletionProtection is set to <value>' — a different sentence that also injects the group name.
Why: the message is the only place AWS tells the operator HOW to proceed (remove deletion protection); the emulator's sentence drops that instruction and cannot be matched.
Fix: the deletion-protection refusal messages in the DeleteAutoScalingGroup handler.
Traces: autoscaling.deep-autoscaling-2-asg-enforce-prevent-all-deletion-rejects-plain-and-force-delete, autoscaling.deep-autoscaling-2-asg-enforce-prevent-force-deletion-rejects-force-delete-of-live-group, autoscaling.deep-autoscaling-2-asg-enforce-prevent-force-deletion-accepts-plain-delete-of-empty-group.

Cause 4 of 9: CancelInstanceRefresh's no-active-refresh sentence is emulator-authored.
AWS: HTTP 400 with Error.Code ActiveInstanceRefreshNotFound and Error.Message 'No in progress, baking, pending, or cancelling Instance Refresh found for Auto Scaling group <group>.' — fixed text including the four-state enumeration and the terminating period; the only variable part is the group name.
Emulator: 'No active Instance Refresh found for Auto Scaling group <group>' — the state enumeration and the final period are gone.
Why: the enumeration is the contract (which refresh states count as cancellable); a client reading the message cannot tell a baking refresh from none at all.
Fix: the message in the CancelInstanceRefresh handler's not-found branch.
Traces: autoscaling.cancel-instance-refresh-not-found-fails.

Cause 5 of 9: StartInstanceRefresh's Preferences cross-field refusals use emulator-authored sentences.
AWS: HTTP 400 / ValidationError with 'The difference between MaxHealthyPercentage and MinHealthyPercentage must be less than or equal to 100.' for an over-100 spread, and 'MinHealthyPercentage is required to use MaxHealthyPercentage. Specify MinHealthyPercentage, or remove MaxHealthyPercentage, and try again.' when the maximum is given alone. Both are fixed text with no variable parts, and no refresh is recorded.
Emulator: 'The difference between MinHealthyPercentage and MaxHealthyPercentage cannot be greater than 100, but MinHealthyPercentage is <min> and MaxHealthyPercentage is <max>' and 'If you specify MaxHealthyPercentage, you must also specify MinHealthyPercentage'.
Why: both emulator sentences omit the remediation AWS states and invert the member order, so neither matches; the refusal class is right but unrecognisable.
Fix: the Preferences validation messages in the StartInstanceRefresh handler.
Traces: autoscaling.instance-refresh-health-spread-over-100-rejected, autoscaling.instance-refresh-max-healthy-without-min-rejected.

Cause 6 of 9: the CheckpointPercentages refusal uses an emulator-authored sentence.
AWS: HTTP 400 / ValidationError with 'Checkpoint percentages must be sorted in ascending order with no duplicates.' — fixed text, no variable parts.
Emulator: 'The CheckpointPercentages values <v1>, <v2>, <v3> are not valid: they must be unique and in ascending order' (it echoes the submitted list).
Why: same class, different text; nothing written against AWS matches it.
Fix: the CheckpointPercentages validation message in the StartInstanceRefresh handler.
Traces: autoscaling.instance-refresh-unsorted-checkpoints-rejected.

Cause 7 of 9: the invalid-NextToken refusal sentence is emulator-authored.
AWS: HTTP 400 with Error.Message "The token '<token>' is invalid." — fixed text 'The token ' + ' is invalid.' with the submitted token single-quoted.
Emulator: "The token '<token>' was invalid" — wrong tense and no terminating period.
Why: pagination error handling keys on this sentence; a retry loop that distinguishes an expired from a malformed token cannot.
Fix: the NextToken decode-failure message shared by the Describe* handlers (observed on DescribeInstanceRefreshes).
Traces: autoscaling.op-contract-describeInstanceRefreshes.

Cause 8 of 9: RollbackInstanceRefresh is an unimplemented scaffold stub.
AWS: HTTP 400 with Error.Code IrreversibleInstanceRefresh and Error.Message "This Auto Scaling group's launch template version isn't compatible with a rollback. Rollbacks are not supported when the Auto Scaling group uses the launch template's '$Latest' or '$Default' version." (fixed text, no variable parts) when the group is pinned to $Latest/$Default, and the same code when there is nothing to roll back.
Emulator: HTTP 501 with Error.Code NotImplemented, 'operation RollbackInstanceRefresh is scaffolded but not implemented in this emulator'.
Why: the whole rollback path is unreachable and the modeled IrreversibleInstanceRefresh error can never be observed; a client's error branch for an unrollbackable group never runs. No open issue names RollbackInstanceRefresh (the stub issues filed cover other operations).
Fix: implement services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/rollback_instance_refresh.rs, including the IrreversibleInstanceRefresh classification above.
Traces: autoscaling.rollback-instance-refresh-irreversible-fails, autoscaling.rollback-instance-refresh-no-active-fails, autoscaling.rollback-without-active-refresh-fails.

Cause 9 of 9: EC2 CreateLaunchTemplate's duplicate-name refusal uses an emulator-authored sentence (exercised from this Auto Scaling suite).
AWS: HTTP 400 with Error.Code InvalidLaunchTemplateName.AlreadyExistsException and Error.Message 'Launch template name already in use.' — fixed text, NO variable parts (the conflicting name is not quoted in the message); the existing template keeps exactly one unaltered version.
Emulator: "The launch template name '<name>' is already in use" (name interpolated, no terminating period).
Why: callers distinguish the launch-template name conflict from the Auto Scaling AlreadyExistsFault boundary by this exact sentence.
Fix: the duplicate-name refusal message in the EC2 CreateLaunchTemplate handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_launch_template.rs).
Traces: autoscaling.create-launch-template-conflicting-name-rejected.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (50/414 passed; emulator up).

Cause 1 — autoscaling.delete-nonexistent-group-fails [mismatch]: expected message "AutoScalingGroup name not found - AutoScalingGroup 'sim2real-behav-as-missing-d43fc92588cc' not found", got 'AutoScalingGroup name not found - AutoScalingGroup: sim2real-behav-as-missing-d43fc92588cc not found'. autoscaling.create-asg-missing-launch-config-rejected [mismatch]: assert dead_message == 'AutoScalingGroup name not found - null' -> got 'AutoScalingGroup name not found - AutoScalingGroup: sim2real-asg-nolc-7705cc826798 not found'. Group names are per-run values.
Cause 2 — autoscaling.launch-config-create-delete-lifecycle [mismatch]: expected 'Launch configuration name not found - A launch configuration with the name: <lc> does not exist', got 'Launch configuration name not found - Launch configuration <lc> not found'.
Cause 3 — autoscaling.deep-autoscaling-2-asg-enforce-prevent-all-deletion-rejects-plain-and-force-delete [mismatch]: 'plain delete under prevent-all-deletion said \'Cannot delete AutoScalingGroup <name> because its DeletionProtection is set to prevent-all-deletion\'' against the check's _PREVENT_ALL_MESSAGE constant quoted in the description; the two prevent-force-deletion checks fail the same way against _PREVENT_FORCE_MESSAGE.
Cause 4 — autoscaling.cancel-instance-refresh-not-found-fails [mismatch]: expected 'No in progress, baking, pending, or cancelling Instance Refresh found for Auto Scaling group <group>.', got 'No active Instance Refresh found for Auto Scaling group <group>'. Code ActiveInstanceRefreshNotFound asserted and correct.
Cause 5 — autoscaling.instance-refresh-health-spread-over-100-rejected [mismatch]: 'the rejection does not state the cross-field rule: \'The difference between MinHealthyPercentage and MaxHealthyPercentage cannot be greater than 100, but MinHealthyPercentage is 49 and MaxHealthyPercentage is 150\''; autoscaling.instance-refresh-max-healthy-without-min-rejected [mismatch]: 'the rejection does not name the missing minimum: \'If you specify MaxHealthyPercentage, you must also specify MinHealthyPercentage\''.
Cause 6 — autoscaling.instance-refresh-unsorted-checkpoints-rejected [mismatch]: assert msg == 'Checkpoint percentages must be sorted in ascending order with no duplicates.' -> got 'The CheckpointPercentages values 50, 20, 100 are not valid: they must be unique and in ascending order'.
Cause 7 — autoscaling.op-contract-describeInstanceRefreshes [mismatch]: expected "The token 'sim2real-not-a-real-token' is invalid.", got "The token 'sim2real-not-a-real-token' was invalid".
Cause 8 — autoscaling.rollback-instance-refresh-irreversible-fails / .rollback-instance-refresh-no-active-fails / .rollback-without-active-refresh-fails [mismatch]: 'expected IrreversibleInstanceRefresh ..., got NotImplemented'.
Cause 9 — autoscaling.create-launch-template-conflicting-name-rejected [mismatch]: assert message == 'Launch template name already in use.' -> got "The launch template name 'sim2real-behav-lt-002681f792e5' is already in use"; the code InvalidLaunchTemplateName.AlreadyExistsException asserted before it passes.

## Steps to Reproduce
1. Replay the behavioral checks in trace_ids against the emulator built from this checkout.
2. By hand:
   a. DeleteAutoScalingGroup for a name that does not exist -> expect "AutoScalingGroup name not found - AutoScalingGroup '<name>' not found"; a group-scoped mutation after a refused create -> 'AutoScalingGroup name not found - null'.
   b. CreateAutoScalingGroup with LaunchConfigurationName=<absent> -> expect 'Launch configuration name not found - A launch configuration with the name: <name> does not exist'.
   c. Set DeletionProtection to prevent-all-deletion, then DeleteAutoScalingGroup (plain and ForceDelete) -> expect the 'You cannot delete this group because its deletion protection is set to ... remove deletion protection for the group and try again.' sentence; repeat with prevent-force-deletion.
   d. CancelInstanceRefresh on a group with no refresh -> expect ActiveInstanceRefreshNotFound with 'No in progress, baking, pending, or cancelling Instance Refresh found for Auto Scaling group <group>.'
   e. StartInstanceRefresh with Preferences {MinHealthyPercentage:49, MaxHealthyPercentage:150}, then {MaxHealthyPercentage:150}, then CheckpointPercentages [50,20,100] -> expect the three AWS sentences quoted above and no refresh recorded.
   f. DescribeInstanceRefreshes with NextToken='not-a-real-token' -> expect "The token 'not-a-real-token' is invalid."
   g. RollbackInstanceRefresh on a $Latest-pinned group -> expect 400 IrreversibleInstanceRefresh with the launch-template-version sentence, not 501 NotImplemented.
   h. EC2 CreateLaunchTemplate twice with one name -> expect InvalidLaunchTemplateName.AlreadyExistsException 'Launch template name already in use.' and the first template still at one version.
