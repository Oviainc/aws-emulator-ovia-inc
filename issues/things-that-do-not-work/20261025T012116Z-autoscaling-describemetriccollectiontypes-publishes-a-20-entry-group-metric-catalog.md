Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.describe-metric-collection-types-static-list autoscaling.describe-static-catalog-endpoints-stable
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.describe-metric-collection-types-static-list|run#6|AssertionError|assert metrics >= expected_metrics, f"missing metrics {expected_metrics - metrics} in {metrics}"
Issue-Key: autoscaling.describe-static-catalog-endpoints-stable|run#19|AssertionError|assert metric_names_1 >= expected_metrics, (
Issue-Commit: 2c888af5aa33aca2a1e4835e22187ff259bac9bc

# DescribeMetricCollectionTypes publishes a 20-entry group-metric catalog, omitting the 5 'retained' / WarmPoolMinSize metrics EnableMetricsCollection accepts

> _Note: this issue was identified against the code base at commit `2c888af5aa33aca2a1e4835e22187ff259bac9bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeMetricCollectionTypes

## Description
ONE net-new root cause on an Auto Scaling path the emulator answers (autoscaling is bootstrapped and serving — the call returns 200 with a well-formed catalog, it is just short). Neither failing check was marked ALREADY FILED or OWNED by the replay report.

OVERLAP NOTE for the fixer: open issue 20261022T175547Z-autoscaling-auto-scaling-4-fidelity-divergences-cloudformation-asg-never.md describes this same catalog gap as its 'Cause 2 of 4' and lists both of these check ids, but its recorded Issue-Key for them pins the OLD assertion text (`assert metrics == expected_metrics`) while the checks now assert `metrics >= expected_metrics`, so the replay key no longer binds and the suite re-reported both as unowned. If that issue's Cause 2 is fixed, this one closes with it — do not fix the catalog twice. The other three causes of that issue (CloudFormation not provisioning ASGs, the ambiguous 'elasticloadbalancing' endpoint prefix, missing SNS notifications) are unrelated to this issue.

Cause 1 of 1: DescribeMetricCollectionTypes returns a stale 20-name group-metric catalog.
AWS: DescribeMetricCollectionTypes (Smithy: input smithy.api#Unit; output members Metrics (MetricCollectionTypes) and Granularities (MetricGranularityTypes); only modeled error ResourceContentionFault, HTTP 500) returns the fixed, account- and region-independent catalog of group metrics that EnableMetricsCollection/DisableMetricsCollection accept, together with the single granularity '1Minute'. The current catalog has 25 entries: the 20 the emulator returns PLUS GroupTerminatingRetainedInstances, GroupTerminatingRetainedCapacity, WarmPoolMinSize, WarmPoolPendingRetainedCapacity and WarmPoolTerminatingRetainedCapacity. Recorded behavior only for those 5 names — the on-disk Smithy model's prose for the metric members still enumerates only the legacy 20, so the model is behind the service here; there is no error path involved, the answer is a 200 either way.
Emulator: Metrics contains exactly 20 entries — GroupMinSize, GroupMaxSize, GroupDesiredCapacity, GroupInServiceInstances, GroupPendingInstances, GroupStandbyInstances, GroupTerminatingInstances, GroupTotalInstances, GroupInServiceCapacity, GroupPendingCapacity, GroupStandbyCapacity, GroupTerminatingCapacity, GroupTotalCapacity, WarmPoolDesiredCapacity, WarmPoolWarmedCapacity, WarmPoolPendingCapacity, WarmPoolTerminatingCapacity, WarmPoolTotalCapacity, GroupAndWarmPoolDesiredCapacity, GroupAndWarmPoolTotalCapacity. The 5 names above are absent. Granularities == {'1Minute'} is correct, and the answer is stable across repeated calls (the stability half of both checks passes).
Why: this catalog is exactly what clients, consoles and IaC validators enumerate to decide which group metrics they may enable. A client that enables a legitimate metric such as GroupTerminatingRetainedInstances or WarmPoolMinSize is told by the emulator that the metric does not exist, and a client that subscribes to 'all group metrics' by walking the catalog silently under-subscribes — its dashboards and alarms are missing the retained-capacity and warm-pool-minimum series with no error anywhere to explain it.
Fix: the catalog data behind the handler, not the handler logic — crate::state::METRIC_COLLECTION_TYPES in services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/state.rs, which services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/describe_metric_collection_types.rs:127 maps straight into the Metrics list. EnableMetricsCollection's / DisableMetricsCollection's accepted-metric validation must be driven from the same constant so the two cannot drift apart.
Traces: autoscaling.describe-metric-collection-types-static-list, autoscaling.describe-static-catalog-endpoints-stable

## Evidence
Check autoscaling.describe-metric-collection-types-static-list [mismatch]:
  autoscaling.describe_metric_collection_types() -> 200, Granularities == {'1Minute'} (correct), Metrics == the 20-name set listed above.
  AssertionError: missing metrics {'WarmPoolPendingRetainedCapacity', 'WarmPoolTerminatingRetainedCapacity', 'WarmPoolMinSize', 'GroupTerminatingRetainedInstances', 'GroupTerminatingRetainedCapacity'} in {'GroupStandbyInstances', 'GroupTerminatingInstances', 'GroupTerminatingCapacity', 'GroupInServiceInstances', 'GroupDesiredCapacity', 'GroupPendingInstances', 'GroupInServiceCapacity', 'WarmPoolTerminatingCapacity', 'GroupAndWarmPoolTotalCapacity', 'GroupMaxSize', 'GroupAndWarmPoolDesiredCapacity', 'GroupMinSize', 'WarmPoolDesiredCapacity', 'WarmPoolPendingCapacity', 'GroupTotalCapacity', 'GroupPendingCapacity', 'GroupStandbyCapacity', 'GroupTotalInstances', 'WarmPoolTotalCapacity', 'WarmPoolWarmedCapacity'}

Check autoscaling.describe-static-catalog-endpoints-stable [mismatch], same cause reached through the combined static-catalog sweep (DescribeAdjustmentTypes, DescribeAutoScalingNotificationTypes, DescribeScalingProcessTypes, DescribeTerminationPolicyTypes and DescribeLifecycleHookTypes all pass; only the metric catalog assertion fails):
  AssertionError: missing metric collection types {'WarmPoolMinSize', 'WarmPoolPendingRetainedCapacity', 'GroupTerminatingRetainedInstances', 'WarmPoolTerminatingRetainedCapacity', 'GroupTerminatingRetainedCapacity'} in {...same 20 names...}

Expected: Metrics is a superset of the 25-name catalog — the 20 returned plus GroupTerminatingRetainedInstances, GroupTerminatingRetainedCapacity, WarmPoolMinSize, WarmPoolPendingRetainedCapacity, WarmPoolTerminatingRetainedCapacity — with Granularities == {'1Minute'} and identical content across repeated calls.

## Steps to Reproduce
Boot the emulator and point a botocore 'autoscaling' client at it (any region; the catalog is account- and region-independent).
1. Call autoscaling.describe_metric_collection_types(). Observe HTTP 200 with exactly 20 entries in Metrics and that GroupTerminatingRetainedInstances, GroupTerminatingRetainedCapacity, WarmPoolMinSize, WarmPoolPendingRetainedCapacity and WarmPoolTerminatingRetainedCapacity are all absent, while Granularities is [{'Granularity': '1Minute'}].
2. Call it a second time and confirm the answer is identical — the defect is the catalog's membership, not its stability.
3. Optionally confirm the knock-on: create an Auto Scaling group and call autoscaling.enable_metrics_collection(AutoScalingGroupName=..., Metrics=['WarmPoolMinSize'], Granularity='1Minute') — a metric real AWS accepts is not in the emulator's catalog, so any client that validates against the catalog first will refuse to send it.
Equivalently, re-replay the behavioral checks listed in trace_ids: autoscaling.describe-metric-collection-types-static-list and autoscaling.describe-static-catalog-endpoints-stable.
