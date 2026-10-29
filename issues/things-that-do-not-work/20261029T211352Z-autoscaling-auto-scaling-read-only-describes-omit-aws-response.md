Issue-Source: trace-sweep-divergence
Issue-Trace: autoscaling-account-level-reads-and-create-validation
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscaling-account-level-reads-and-create-validation autoscaling-account-limits-isolated-read autoscaling-reference-data-catalog autoscaling-catalog-describe-auto-scaling-notification-types autoscaling-static-catalogs-and-unfiltered-describes
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|DescribeAccountLimits|2xx->2xx|$.DescribeAccountLimitsResponse.DescribeAccountLimitsResult.ThrottlingLimits
Issue-Key: autoscaling|DescribeAutoScalingNotificationTypes|2xx->2xx|$.DescribeAutoScalingNotificationTypesResponse.DescribeAutoScalingNotificationTypesResult.AutoScalingNotificationTypes.member[*]
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# Auto Scaling read-only describes omit AWS response data: DescribeAccountLimits ThrottlingLimits and two notification types

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAccountLimits

## Description
Two net-new causes, one fault: an Auto Scaling read-only describe answers 200 but its payload is short of the data AWS serves. A fixer must close BOTH before submitting.

Cause 1 of 2 — DescribeAccountLimits omits ThrottlingLimits. AWS: HTTP 200 with DescribeAccountLimitsResponse/DescribeAccountLimitsResult carrying, besides the four count members, a ThrottlingLimits structure with AccountMaxCallRate{RefillRate, BucketSize} describing the account's API call-rate bucket (the recorded run reports RefillRate 5.0 and BucketSize 30; those are account/region values, the requirement is that the member is present and both sub-members are serialized as the numeric strings AWS sends). Smithy: DescribeAccountLimitsAnswer models only MaxNumberOfAutoScalingGroups, MaxNumberOfLaunchConfigurations, NumberOfAutoScalingGroups and NumberOfLaunchConfigurations, so ThrottlingLimits is recorded behavior only — the live awsQuery response carries it and the model does not. Emulator: 200 with the four counts and no ThrottlingLimits element at all. Why: a client reading its own throttle budget (or an SDK shape that models the member) gets a null where AWS always gives a rate, so capacity/throttle logic that consults it cannot run against the emulator. Fix: the DescribeAccountLimits handler/serializer must emit the ThrottlingLimits subtree from the account limits state.

Cause 2 of 2 — DescribeAutoScalingNotificationTypes serves an incomplete catalog in the wrong order. AWS: HTTP 200 with seven AutoScalingNotificationTypes members in a fixed order, the last three being autoscaling:EC2_ROOT_VOLUME_REPLACEMENT, autoscaling:EC2_ROOT_VOLUME_REPLACEMENT_ERROR and autoscaling:TEST_NOTIFICATION. Smithy: DescribeAutoScalingNotificationTypesAnswer.AutoScalingNotificationTypes is an untyped string list, so the membership is recorded behavior only; the recording shows the two EC2_ROOT_VOLUME_REPLACEMENT* types present and TEST_NOTIFICATION last. Emulator: returns only five members — it places autoscaling:TEST_NOTIFICATION at index 4 and stops, so both root-volume-replacement types are missing entirely. Why: this catalog is what a console or client offers when wiring PutNotificationConfiguration; the two missing types can never be selected, and a caller validating a NotificationType against the catalog rejects a type AWS accepts. Fix: the DescribeAutoScalingNotificationTypes static catalog (the notification-type seed in the autoscaling service state/handler).

The DescribeAutoScalingGroups projection gaps seen in the same fixtures are already owned by 20261022T112459Z-autoscaling-group-round-trip-create-skips-min-max.md and are NOT part of this issue.

## Evidence
Replay of simparity-wire-aws-v1 against the booted emulator (emulator_up true):
Cause 1 — DescribeAccountLimits, status 200 -> 200: $.DescribeAccountLimitsResponse.DescribeAccountLimitsResult.ThrottlingLimits: missing (expected {'AccountMaxCallRate': {'RefillRate': '5.0', 'BucketSize': '30'}}, got None). Cluster of 3 steps: autoscaling-account-level-reads-and-create-validation#task#1, autoscaling-account-limits-isolated-read#task#1, autoscaling-reference-data-catalog#task#1.
Cause 2 — DescribeAutoScalingNotificationTypes, status 200 -> 200, two clusters:
  (a) ...AutoScalingNotificationTypes.member[4]: missing (expected 'autoscaling:EC2_ROOT_VOLUME_REPLACEMENT', got 'autoscaling:TEST_NOTIFICATION'); member[5]: missing (expected 'autoscaling:EC2_ROOT_VOLUME_REPLACEMENT_ERROR', got None) — autoscaling-catalog-describe-auto-scaling-notification-types#task#1 and autoscaling-reference-data-catalog#task#5.
  (b) member[4]: value_mismatch (expected 'autoscaling:EC2_ROOT_VOLUME_REPLACEMENT', got 'autoscaling:TEST_NOTIFICATION'); member[5]: missing (expected 'autoscaling:EC2_ROOT_VOLUME_REPLACEMENT_ERROR', got None); member[6]: missing (expected 'autoscaling:TEST_NOTIFICATION', got None) — autoscaling-static-catalogs-and-unfiltered-describes#task#1.
Smithy grounding (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json): DescribeAccountLimits input Unit, output DescribeAccountLimitsAnswer {MaxNumberOfAutoScalingGroups, MaxNumberOfLaunchConfigurations, NumberOfAutoScalingGroups, NumberOfLaunchConfigurations} — no ThrottlingLimits member, so that subtree is recorded behavior only; DescribeAutoScalingNotificationTypes output AutoScalingNotificationTypes (string list, membership recorded behavior only).

## Steps to Reproduce
1) Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2) Cause 1: POST Action=DescribeAccountLimits&Version=2011-01-01 -> inspect DescribeAccountLimitsResult for a ThrottlingLimits/AccountMaxCallRate element with RefillRate and BucketSize (autoscaling-account-level-reads-and-create-validation#task#1).
3) Cause 2: POST Action=DescribeAutoScalingNotificationTypes&Version=2011-01-01 -> count members; expect seven, with autoscaling:EC2_ROOT_VOLUME_REPLACEMENT and autoscaling:EC2_ROOT_VOLUME_REPLACEMENT_ERROR before autoscaling:TEST_NOTIFICATION (autoscaling-catalog-describe-auto-scaling-notification-types#task#1, autoscaling-static-catalogs-and-unfiltered-describes#task#1).
