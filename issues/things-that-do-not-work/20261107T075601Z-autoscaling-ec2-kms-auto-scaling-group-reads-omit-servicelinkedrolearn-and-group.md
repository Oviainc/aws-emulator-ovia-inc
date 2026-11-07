Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__kms.asglf-group-launch-under-disabled-key-blocked-until-key-enabled autoscaling__ec2__kms.asglf-group-launch-under-enabled-key-without-slr-grant-blocked autoscaling__ec2__kms.asglf-desired-or-min-above-max-rejected autoscaling__ec2__kms.asglf-cli-diagnosis-derives-like-the-sdk autoscaling__ec2__kms.asglf-group-round-trips-bounds-template-reference-and-defaults autoscaling__ec2__kms.asglf-group-on-absent-template-version-rejected autoscaling__ec2__kms.asglf-rebuilding-template-and-group-rejected-as-duplicates autoscaling__ec2__kms.asglf-zero-desired-group-records-no-launch-attempt-negative-delivery
Issue-Key: autoscaling__ec2__kms.asglf-group-launch-under-disabled-key-blocked-until-key-enabled|run#8|KeyError|_grant_slr(ctx, key["KeyId"], _group(ctx, name)["ServiceLinkedRoleARN"])
Issue-Key: autoscaling__ec2__kms.asglf-group-launch-under-enabled-key-without-slr-grant-blocked|run#10|KeyError|slr = _group(ctx, name)["ServiceLinkedRoleARN"]
Issue-Key: autoscaling__ec2__kms.asglf-desired-or-min-above-max-rejected|run#7|AssertionError|assert msg == message, f"{label} answered message {msg!r}, expected {message!r}"
Issue-Key: autoscaling__ec2__kms.asglf-cli-diagnosis-derives-like-the-sdk|run#11|AssertionError|assert tmpl.split() == [lt["LaunchTemplateName"], "1"], f"CLI template rendered {tmpl!r}"
Issue-Key: autoscaling__ec2__kms.asglf-group-round-trips-bounds-template-reference-and-defaults|run#9|AssertionError|assert declared == {
Issue-Key: autoscaling__ec2__kms.asglf-group-on-absent-template-version-rejected|run#11|AssertionError|assert message == expected_message, f"EC2 version 99 lookup message: {message!r}"
Issue-Key: autoscaling__ec2__kms.asglf-rebuilding-template-and-group-rejected-as-duplicates|run#13|AssertionError|assert message == "Launch template name already in use.", (
Issue-Key: autoscaling__ec2__kms|DescribeScalingActivities|NotImplemented
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# Auto Scaling group reads omit ServiceLinkedRoleARN and group defaults; 5 further refusal-wording/representation causes

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
Seven net-new root causes behind the surviving autoscaling__ec2__kms failures.

Cause 1 of 7: DescribeAutoScalingGroups omits ServiceLinkedRoleARN.
AWS: Smithy autoscaling AutoScalingGroup.ServiceLinkedRoleARN (ResourceName) — "The Amazon Resource Name (ARN) of the service-linked role that the Auto Scaling group uses to call other Amazon Web Services on your behalf" — is always present; when the create did not name one it is the account's AWSServiceRoleForAutoScaling ARN.
Emulator: the member is absent from the described group, so a client indexing it raises KeyError.
Why: any caller that reads the group's role (to grant it a KMS key, to diagnose an encrypted-volume launch failure — exactly these checks) cannot proceed at all.
Fix: AutoScalingGroup state/serializer — default and always emit ServiceLinkedRoleARN.
Traces: autoscaling__ec2__kms.asglf-group-launch-under-disabled-key-blocked-until-key-enabled, autoscaling__ec2__kms.asglf-group-launch-under-enabled-key-without-slr-grant-blocked.

Cause 2 of 7: SetDesiredCapacity's above-max refusal appends the group name to a fixed AWS sentence.
AWS: ValidationError with exactly "New SetDesiredCapacity value <n> is above max value <max> for the AutoScalingGroup." — fixed text ending in a period after "AutoScalingGroup"; variable parts are the requested value and the group's MaxSize.
Emulator: "New SetDesiredCapacity value 2 is above max value 1 for the AutoScalingGroup: <group name>" — the period is replaced by ": <name>".
Why: clients match this sentence to classify the bounds error; the trailing name breaks equality and any regex anchored at the period.
Fix: SetDesiredCapacity validation message.
Traces: autoscaling__ec2__kms.asglf-desired-or-min-above-max-rejected.

Cause 3 of 7: the group's launch-template reference reads back with no LaunchTemplateName, so CLI projections render None.
AWS: the stored reference carries both LaunchTemplateId and LaunchTemplateName (Smithy autoscaling LaunchTemplateSpecification) regardless of which one the create supplied.
Emulator: the by-id group has no LaunchTemplateName, so `--query 'AutoScalingGroups[].LaunchTemplate.[LaunchTemplateName,Version]'` renders "None\t1".
Why: the documented CLI diagnosis of a launch failure (name + version) is unusable; SDK and CLI disagree with AWS.
Fix: AutoScalingGroup.LaunchTemplate resolution in state (same resolution as Cause 3 of the autoscaling__ec2 issue, reached here through the CLI path).
Traces: autoscaling__ec2__kms.asglf-cli-diagnosis-derives-like-the-sdk.

Cause 4 of 7: a created group does not round-trip its declared and derived values.
AWS: DescribeAutoScalingGroups returns the declared MinSize/MaxSize/DesiredCapacity/VPCZoneIdentifier/LaunchTemplate plus the required derived members — AvailabilityZones, DefaultCooldown, HealthCheckType, CreatedTime are marked required in the Smithy AutoScalingGroup shape, and the AZ set is derived from the subnets.
Emulator: the described group does not match the declared/derived set the check asserts (the group it returns carries the declared bounds and subnet but not the full derived shape).
Why: a client reading the group back cannot recover what it asked for, and required members are missing from a modelled response.
Fix: CreateAutoScalingGroup -> state projection and the DescribeAutoScalingGroups serializer.
Traces: autoscaling__ec2__kms.asglf-group-round-trips-bounds-template-reference-and-defaults.

Cause 5 of 7: EC2's absent-launch-template-version refusal message is emulator-authored.
AWS: refusing a group whose Version names a version that does not exist, EC2 answers InvalidLaunchTemplateId.VersionNotFound with AWS's sentence for the missing version rather than the emulator's prose.
Emulator: "Launch template version '99' does not exist for launch template 'lt-...'".
Why: the message is the only place that identifies which half of the reference is wrong; a caller matching AWS's text cannot classify it.
Fix: EC2 launch-template-version lookup error message (DescribeLaunchTemplateVersions / the ASG create path that validates it).
Traces: autoscaling__ec2__kms.asglf-group-on-absent-template-version-rejected.

Cause 6 of 7: the duplicate launch-template-name refusal message is emulator-authored.
AWS: CreateLaunchTemplate with a name already in use is refused InvalidLaunchTemplateName.AlreadyExistsException with AWS's exact conflicting-name sentence (fixed text + the name from the request).
Emulator: "The launch template name 'sim2real-asglf-lt-...' is already in use".
Why: a create-if-absent client keys on the documented message to decide the name is taken; it sees an unknown string.
Fix: CreateLaunchTemplate duplicate-name validation message.
Traces: autoscaling__ec2__kms.asglf-rebuilding-template-and-group-rejected-as-duplicates.

Cause 7 of 7: DescribeScalingActivities is a scaffold stub, so a zero-desired group cannot be shown to have recorded no launch attempt.
AWS: DescribeScalingActivities returns an empty Activities list for a group that never launched — the negative evidence the scenario needs.
Emulator: NotImplemented for every call.
Why: there is no way to assert "nothing was attempted", and no way to read a failed launch's Cause/StatusCode.
Fix: services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/describe_scaling_activities.rs plus the activity records the launch path should write.
Traces: autoscaling__ec2__kms.asglf-zero-desired-group-records-no-launch-attempt-negative-delivery.

## Evidence
From the behavioral replay:
- asglf-group-launch-under-disabled-key-blocked-until-key-enabled [error] and asglf-group-launch-under-enabled-key-without-slr-grant-blocked [error]: KeyError: 'ServiceLinkedRoleARN'.
- asglf-desired-or-min-above-max-rejected [mismatch]: set_desired_capacity(2) answered message 'New SetDesiredCapacity value 2 is above max value 1 for the AutoScalingGroup: sim2real-asglf-asg-...', expected 'New SetDesiredCapacity value 2 is above max value 1 for the AutoScalingGroup.'.
- asglf-cli-diagnosis-derives-like-the-sdk [mismatch]: AssertionError: CLI template rendered 'None\t1'.
- asglf-group-round-trips-bounds-template-reference-and-defaults [mismatch]: declared/derived values did not round-trip: {'MinSize': 0, 'MaxSize': 1, 'DesiredCapacity': 0, 'VPCZoneIdentifier': 'subnet-...', 'LaunchTemplate': {'LaunchTemplateId': 'lt-...', 'Version': '1'}, 'AvailabilityZones': ['us-west-2a']}.
- asglf-group-on-absent-template-version-rejected [mismatch]: EC2 version 99 lookup message: "Launch template version '99' does not exist for launch template 'lt-...'".
- asglf-rebuilding-template-and-group-rejected-as-duplicates [mismatch]: expected the exact conflicting-name message, got "The launch template name '...' is already in use".
- asglf-zero-desired-group-records-no-launch-attempt-negative-delivery [error]: ClientError (NotImplemented) calling DescribeScalingActivities.

## Steps to Reproduce
Replay the behavioral suite and re-run the checks in trace_ids: each creates a KMS key, a launch template with an encrypted root mapping and an Auto Scaling group, then reads the group back (ServiceLinkedRoleARN, launch-template reference, derived defaults), exercises SetDesiredCapacity past MaxSize, re-creates the template/group for the duplicate-name refusals, and calls DescribeScalingActivities.
