Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.delete-asg-in-use-without-force-fails autoscaling.set-desired-capacity-launches-instances
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.delete-asg-in-use-without-force-fails|run#36|AssertionError|assert dead.get("Error", {}).get("Message") == "AutoScalingGroup name not found - null", (
Issue-Key: auto-scaling|UpdateAutoScalingGroup|ValidationError
Issue-Commit: 024d850c8394c795adbf7fe56e44953224a0a9c6

# UpdateAutoScalingGroup on an absent group returns a ValidationError message that interpolates the group name instead of AWS's literal "AutoScalingGroup name not found - null"

> _Note: this issue was identified against the code base at commit `024d850c8394c795adbf7fe56e44953224a0a9c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAutoScalingGroup

## Description
One root cause, grounded in 2 failing behavioral checks.

Cause 1 of 1: UpdateAutoScalingGroup's not-found ValidationError message is built by interpolating the requested group name, but AWS emits a fixed message whose trailing detail field is the literal token `null`.

AWS: The Smithy model binds UpdateAutoScalingGroup only to ResourceContentionFault (500), ScalingActivityInProgressFault (400) and ServiceLinkedRoleFailure (500) — no not-found error is modeled, so an unknown AutoScalingGroupName is refused with the service-common query-protocol `ValidationError` at HTTP 400. The model says nothing about the message text, so: recorded behavior only. The recording shows AWS answering HTTP 400, Code `ValidationError`, Message exactly `AutoScalingGroup name not found - null`. Every part of that message is FIXED text: `AutoScalingGroup name not found - ` plus the literal `null`. Notably the detail field after the dash is NOT filled from the request — AWS leaves it as `null` for UpdateAutoScalingGroup, even though sibling Auto Scaling operations do interpolate a resource name there. No part of this message is a value taken from the request or from emulator state.

Emulator: answers Code `ValidationError` at an HTTP 4xx (those parts are correct), but with Message `AutoScalingGroup name not found - AutoScalingGroup: <AutoScalingGroupName> not found` — it substitutes the requested group name into the detail field and adds an `AutoScalingGroup: ` prefix and a trailing ` not found`. Replay output: `AssertionError: expected the exact ValidationError message AWS returns, got 'AutoScalingGroup name not found - AutoScalingGroup: sim2real-behav-as-grp-4e8c56f110b4 not found'` (and the same with `...-f1fa58671980`, confirming the varying segment is the request's group name, not a fixed string).

Why: a client that asserts on, matches, or logs the not-found message for UpdateAutoScalingGroup sees text AWS never produces. Because the emulator's variant embeds a per-request group name, the message is also non-deterministic across runs where AWS's is constant, so any exact-match or regex guard a caller writes against the real service (a deletion-confirmation probe, an idempotent-teardown path that distinguishes "already gone" from other ValidationErrors, or a test asserting the post-delete tombstone) fails against the emulator. Both failing checks are positive-control teardown assertions: they delete a group and then confirm a mutation on the dead name is refused the way AWS refuses it.

Fix: the not-found refusal in the UpdateAutoScalingGroup handler, services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/update_auto_scaling_group.rs — the `state.groups.get(&name)` miss branch must emit the fixed message rather than formatting the name into it. Note the neighbouring launch-configuration miss in the same handler uses the same interpolating pattern; only the AutoScalingGroupName branch is grounded in a failing check here, so verify the launch-configuration wording against AWS separately rather than changing it blind.

Traces: autoscaling.delete-asg-in-use-without-force-fails (its Beat-3 assertion, after the plain delete of the emptied group succeeds), autoscaling.set-desired-capacity-launches-instances (its `_expect_not_found` helper on the deleted group). No open issue covers this message; the nearest open items (20261022T112459Z-autoscaling-group-round-trip-create-skips-min-max.md, 20261024T032514Z-autoscaling-setdesiredcapacity-answers-501-notimplemented-for-a-vpczoneidentifier-only.md) concern create-time validation and a 501 on SetDesiredCapacity, not the UpdateAutoScalingGroup not-found message.

## Evidence
Expected (AWS, from the recording): HTTP 400; Error.Code = `ValidationError`; Error.Message = `AutoScalingGroup name not found - null` (entirely fixed text; the detail after the dash is the literal `null`, not the group name).

Actual (emulator): HTTP 4xx; Error.Code = `ValidationError` (correct); Error.Message = `AutoScalingGroup name not found - AutoScalingGroup: <requested-group-name> not found`.

Replay assertions:
- autoscaling.delete-asg-in-use-without-force-fails, check_autoscaling_delete_asg_in_use_without_force_fails.py:270 ->
  `AssertionError: expected the exact ValidationError message AWS returns, got 'AutoScalingGroup name not found - AutoScalingGroup: sim2real-behav-as-grp-4e8c56f110b4 not found'`
- autoscaling.set-desired-capacity-launches-instances, check_autoscaling_set_desired_capacity_launches_instances.py:187 (via _expect_not_found at line 275) ->
  `AssertionError: expected the exact ValidationError message AWS returns, got 'AutoScalingGroup name not found - AutoScalingGroup: sim2real-behav-as-grp-f1fa58671980 not found'`

The two differing group names across the two checks show the diverging segment is interpolated from the request, which is exactly what AWS does not do here.

Source of the divergence: services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/update_auto_scaling_group.rs:409-413 — the `let Some(group) = state.groups.get(&name) else` branch returns `Scaffolded::validation_error(format!("AutoScalingGroup name not found - AutoScalingGroup: {name} not found"))`.

Smithy: UpdateAutoScalingGroup errors = ResourceContentionFault (awsQueryError ResourceContention, 500), ScalingActivityInProgressFault (ScalingActivityInProgress, 400), ServiceLinkedRoleFailure (500). No not-found error is modeled, so the refusal is the service-common ValidationError at 400; the model does not constrain the message text.

## Steps to Reproduce
1. Create an Auto Scaling group backed by a launch template (MinSize 0, MaxSize 1, DesiredCapacity 0 is enough; the group need only exist).
2. Delete it so the name is absent: DeleteAutoScalingGroup(AutoScalingGroupName=<name>) — with ForceDelete=true, or after scaling to 0 — and poll DescribeAutoScalingGroups until the group is gone.
3. Call UpdateAutoScalingGroup(AutoScalingGroupName=<name>, MaxSize=2) on the now-absent name.
4. Inspect the ClientError: Error.Code is `ValidationError` and the status is 400 as AWS returns, but Error.Message reads `AutoScalingGroup name not found - AutoScalingGroup: <name> not found` instead of AWS's fixed `AutoScalingGroup name not found - null`.
5. Repeat with a different group name and observe the message changes with the name, whereas AWS's message is constant.

Replay to confirm a fix: autoscaling.delete-asg-in-use-without-force-fails and autoscaling.set-desired-capacity-launches-instances.
