Issue-Source: trace-sweep-divergence
Issue-Trace: autoscaling-account-level-reads-and-create-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscaling-account-level-reads-and-create-validation autoscaling-account-limits-isolated-read autoscaling-reference-data-catalog autoscaling-catalog-describe-scaling-process-types autoscaling-catalog-describe-termination-policy-types autoscaling-catalog-describe-adjustment-types autoscaling-launch-config-and-account-level-policy-reads autoscaling-catalog-describe-auto-scaling-notification-types autoscaling-static-catalogs-and-unfiltered-describes autoscaling-catalog-describe-lifecycle-hook-types autoscaling-catalog-describe-metric-collection-types
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|DescribeAccountLimits|2xx->4xx|$.DescribeAccountLimitsResponse
Issue-Key: autoscaling|DescribeAccountLimits|2xx->4xx|status
Issue-Key: autoscaling|DescribeScalingProcessTypes|2xx->4xx|$.DescribeScalingProcessTypesResponse
Issue-Key: autoscaling|DescribeScalingProcessTypes|2xx->4xx|status
Issue-Key: autoscaling|DescribeTerminationPolicyTypes|2xx->4xx|$.DescribeTerminationPolicyTypesResponse
Issue-Key: autoscaling|DescribeTerminationPolicyTypes|2xx->4xx|status
Issue-Key: autoscaling|DescribeAdjustmentTypes|2xx->4xx|$.DescribeAdjustmentTypesResponse
Issue-Key: autoscaling|DescribeAdjustmentTypes|2xx->4xx|status
Issue-Key: autoscaling|DescribeAutoScalingNotificationTypes|2xx->4xx|$.DescribeAutoScalingNotificationTypesResponse
Issue-Key: autoscaling|DescribeAutoScalingNotificationTypes|2xx->4xx|status
Issue-Key: autoscaling|DescribeLifecycleHookTypes|2xx->4xx|$.DescribeLifecycleHookTypesResponse
Issue-Key: autoscaling|DescribeLifecycleHookTypes|2xx->4xx|status
Issue-Key: autoscaling|DescribeMetricCollectionTypes|2xx->4xx|$.DescribeMetricCollectionTypesResponse
Issue-Key: autoscaling|DescribeMetricCollectionTypes|2xx->4xx|status
Issue-Commit: 6f223cb07d8f19a9e48b2b4f7aac2665ac919e98

# autoscaling static catalog and account-limit describes all answer 400 instead of returning the fixed catalog

> _Note: this issue was identified against the code base at commit `6f223cb07d8f19a9e48b2b4f7aac2665ac919e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAccountLimits

## Description
Every zero-input, read-only "reference data" operation of EC2 Auto Scaling is rejected with HTTP 400 instead of returning its fixed catalog. All seven causes below are the same fault (these operations are not served at all, so the query dispatcher falls through to a validation rejection) and all seven must be closed before this issue is submitted. Smithy (com.amazonaws.autoscaling, awsQuery, version 2011-01-01): each of these operations has input_shape Unit — there is no input member a client can get wrong — and the ONLY error bound to each of them is ResourceContentionFault (awsQueryError code ResourceContention, HTTP 500). No 4xx error is modeled for any of them, so a 400 can never be a correct answer to a well-formed request.

Cause 1 of 7: DescribeAccountLimits. AWS: 200 with DescribeAccountLimitsResult carrying MaxNumberOfAutoScalingGroups, MaxNumberOfLaunchConfigurations, NumberOfAutoScalingGroups, NumberOfLaunchConfigurations (recording also shows the ThrottlingLimits/AccountMaxCallRate block); Smithy: input Unit, errors = ResourceContentionFault only. Emulator: HTTP 400 and no DescribeAccountLimitsResponse element at all. Why: clients that read account limits before creating a group (and every capacity-planning/console-style flow) get a hard client error where AWS reports quota numbers, so they conclude the account is misconfigured rather than proceeding. Fix: the DescribeAccountLimits handler in the autoscaling crate (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01) — it must serve a counted response from state (group/launch-configuration counts) plus the static maxima. Traces: autoscaling-account-level-reads-and-create-validation#task#1 (also autoscaling-account-limits-isolated-read#task#1, autoscaling-reference-data-catalog#task#1).

Cause 2 of 7: DescribeScalingProcessTypes. AWS: 200 with the fixed nine process names (AZRebalance, AddToLoadBalancer, AlarmNotification, HealthCheck, InstanceRefresh, Launch, ReplaceUnhealthy, ScheduledActions, Terminate); Smithy: input Unit, errors = ResourceContentionFault only. Emulator: HTTP 400, no DescribeScalingProcessTypesResponse. Why: a client validating a SuspendProcesses/ResumeProcesses argument against the service's own list cannot obtain it. Fix: the DescribeScalingProcessTypes handler — return the constant catalog. Traces: autoscaling-account-level-reads-and-create-validation#task#2 (also autoscaling-catalog-describe-scaling-process-types#task#1, autoscaling-reference-data-catalog#task#6).

Cause 3 of 7: DescribeTerminationPolicyTypes. AWS: 200 with the fixed list (AllocationStrategy, ClosestToNextInstanceHour, Default, Lambda, NewestInstance, OldestInstance, OldestLaunchConfiguration, OldestLaunchTemplate); Smithy: input Unit, errors = ResourceContentionFault only. Emulator: HTTP 400, no DescribeTerminationPolicyTypesResponse. Why: TerminationPolicies values cannot be discovered or validated. Fix: the DescribeTerminationPolicyTypes handler — return the constant catalog. Traces: autoscaling-account-level-reads-and-create-validation#task#3 (also autoscaling-catalog-describe-termination-policy-types#task#1, autoscaling-reference-data-catalog#task#2).

Cause 4 of 7: DescribeAdjustmentTypes. AWS: 200 with ChangeInCapacity, ExactCapacity, PercentChangeInCapacity; Smithy: input Unit, errors = ResourceContentionFault only. Emulator: HTTP 400, no DescribeAdjustmentTypesResponse. Why: a client cannot validate a scaling policy's AdjustmentType against the service. Fix: the DescribeAdjustmentTypes handler — return the constant catalog. Traces: autoscaling-catalog-describe-adjustment-types#task#1 (also autoscaling-launch-config-and-account-level-policy-reads#task#4, autoscaling-reference-data-catalog#task#3).

Cause 5 of 7: DescribeAutoScalingNotificationTypes. AWS: 200 with the seven autoscaling:EC2_INSTANCE_* / TEST_NOTIFICATION values; Smithy: input Unit, errors = ResourceContentionFault only. Emulator: HTTP 400, no DescribeAutoScalingNotificationTypesResponse. Why: notification wiring (PutNotificationConfiguration) cannot be validated against the service's own type list. Fix: the DescribeAutoScalingNotificationTypes handler — return the constant catalog. Traces: autoscaling-catalog-describe-auto-scaling-notification-types#task#1 (also autoscaling-reference-data-catalog#task#5, autoscaling-static-catalogs-and-unfiltered-describes#task#1).

Cause 6 of 7: DescribeLifecycleHookTypes. AWS: 200 with autoscaling:EC2_INSTANCE_LAUNCHING and autoscaling:EC2_INSTANCE_TERMINATING; Smithy: input Unit, errors = ResourceContentionFault only. Emulator: HTTP 400, no DescribeLifecycleHookTypesResponse. Why: lifecycle-hook transitions cannot be discovered. Fix: the DescribeLifecycleHookTypes handler — return the constant catalog. Traces: autoscaling-catalog-describe-lifecycle-hook-types#task#1 (also autoscaling-reference-data-catalog#task#4, autoscaling-static-catalogs-and-unfiltered-describes#task#2).

Cause 7 of 7: DescribeMetricCollectionTypes. AWS: 200 with the Group*/WarmPool* metric list and Granularities = 1Minute; Smithy: input Unit, errors = ResourceContentionFault only. Emulator: HTTP 400, no DescribeMetricCollectionTypesResponse. Why: EnableMetricsCollection callers cannot discover the legal Metrics/Granularity values. Fix: the DescribeMetricCollectionTypes handler — return the constant catalog (metric list plus the single granularity). Traces: autoscaling-catalog-describe-metric-collection-types#task#1 (also autoscaling-launch-config-and-account-level-policy-reads#task#5, autoscaling-reference-data-catalog#task#7).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline. All seven clusters: expected_status 200, actual_status 400, and the whole top-level <Op>Response element reported missing ("got None").

Cause 1 — DescribeAccountLimits, autoscaling-account-level-reads-and-create-validation#task#1: expected 200 with {'DescribeAccountLimitsResult': {'NumberOfLaunchConfigurations': '0', 'ThrottlingLimits': {'AccountMaxCallRate': {'RefillRate': '5.0', 'BucketSize': '30'}}, 'MaxNumberOfAutoScalingGroups': '500', 'MaxNumberOfLaunchConfigurations': '200', 'NumberOfAutoScalingGroups': '0'}}; got 400, $.DescribeAccountLimitsResponse missing. 3 fixtures affected.
Cause 2 — DescribeScalingProcessTypes, #task#2: expected 200 with 9 ProcessName members; got 400, $.DescribeScalingProcessTypesResponse missing. 3 fixtures.
Cause 3 — DescribeTerminationPolicyTypes, #task#3: expected 200 with 8 TerminationPolicyTypes members; got 400, response element missing. 3 fixtures.
Cause 4 — DescribeAdjustmentTypes, autoscaling-catalog-describe-adjustment-types#task#1: expected 200 with 3 AdjustmentType members; got 400, response element missing. 3 fixtures.
Cause 5 — DescribeAutoScalingNotificationTypes, autoscaling-catalog-describe-auto-scaling-notification-types#task#1: expected 200 with 7 notification types; got 400, response element missing. 3 fixtures.
Cause 6 — DescribeLifecycleHookTypes, autoscaling-catalog-describe-lifecycle-hook-types#task#1: expected 200 with 2 hook types; got 400, response element missing. 3 fixtures.
Cause 7 — DescribeMetricCollectionTypes, autoscaling-catalog-describe-metric-collection-types#task#1: expected 200 with 25 Metric members + Granularity 1Minute; got 400, response element missing. 3 fixtures.

Model evidence (smithy_query, services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json): each of the seven operations has input_shape "Unit" and errors == [ResourceContentionFault / ResourceContention / 500]; no client-side 400 error is bound to any of them.

## Steps to Reproduce
1. Boot the emulator from mainline and point an awsQuery (autoscaling 2011-01-01) client at the data listener.
2. Replay fixture autoscaling-account-level-reads-and-create-validation, step task#1 (Action=DescribeAccountLimits, no other parameters): observe HTTP 400 with no DescribeAccountLimitsResponse instead of HTTP 200 with DescribeAccountLimitsResult.
3. Repeat for each remaining cause: autoscaling-account-level-reads-and-create-validation#task#2 (DescribeScalingProcessTypes) and #task#3 (DescribeTerminationPolicyTypes), autoscaling-catalog-describe-adjustment-types#task#1, autoscaling-catalog-describe-auto-scaling-notification-types#task#1, autoscaling-catalog-describe-lifecycle-hook-types#task#1, autoscaling-catalog-describe-metric-collection-types#task#1 — each returns 400 with no response element.
4. After fixing, re-replay all seven fixture#step bindings listed above (every one of them, not just task#1) plus the sibling fixtures autoscaling-reference-data-catalog and autoscaling-static-catalogs-and-unfiltered-describes.
