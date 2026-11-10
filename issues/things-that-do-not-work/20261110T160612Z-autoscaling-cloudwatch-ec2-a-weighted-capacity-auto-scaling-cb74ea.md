Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudwatch__ec2.weighted-capacity-group-metrics-diverge-from-instance-counts
Issue-Key: autoscaling__cloudwatch__ec2.weighted-capacity-group-metrics-diverge-from-instance-counts|wait#8|Timeout|self._event.wait(delay)
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# autoscaling__cloudwatch__ec2: a weighted-capacity Auto Scaling group never reaches an InService fleet, so the weighted capacity-vs-instance metric comparison never reaches a verdict

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
Cause 1 of 1: a group whose launch overrides carry WeightedCapacity never converges to its InService fleet and its AWS/AutoScaling Group* series never becomes readable, so the check runs out its whole budget with no comparison performed.

AWS:      a group with MinSize=0, MaxSize=4 and DesiredCapacity=4 units whose only override has WeightedCapacity=2 launches 2 instances and settles; DescribeAutoScalingGroups then reports 2 InService instances, and the AWS/AutoScaling namespace carries the capacity family (GroupDesiredCapacity=4, GroupInServiceCapacity=4, GroupTotalCapacity=4) alongside the instance family (GroupInServiceInstances=2, GroupTotalInstances=2) — the two families differ by exactly the weight. Recorded behavior only for the metric names (AWS/AutoScaling group metrics are documented, not Smithy shapes).
Emulator: the check produces no verdict at all: `Error: Check timed out after 600.0s`. Its only unbounded waits are (a) the group reaching DesiredCapacity/Weight InService instances and (b) GetMetricStatistics returning the AWS/AutoScaling Group* datapoints. Hypothesis: the weighted group never reports an InService instance, so the poll never terminates — the same symptom the open issue 20261109T172356Z-autoscaling-cloudwatch-ec2-a-weighted-capacity-auto-scaling.md records for this scenario under a replay key that no longer matches this run's failure.
Why:      weighted capacity is how customers mix instance sizes in one group; if a weighted group never becomes InService, every mixed-instances group is dead and the capacity-vs-instance metric distinction (what autoscaling alarms key off) is unobservable.
Fix:      the Auto Scaling group convergence component (the reconciler that launches instances for DesiredCapacity and marks them InService) with WeightedCapacity accounted in units rather than instances, and the group-metric publisher that feeds the AWS/AutoScaling namespace.
Traces:   autoscaling__cloudwatch__ec2.weighted-capacity-group-metrics-diverge-from-instance-counts.

## Evidence
Expected (AWS): within the check's 600 s budget the group reports 2 InService instances and GetMetricStatistics over AWS/AutoScaling returns GroupDesiredCapacity=4.0, GroupInServiceInstances=2.0, GroupInServiceCapacity=4.0, GroupTotalInstances=2.0, GroupTotalCapacity=4.0, GroupPendingCapacity=0.0.
Actual (emulator): `Error: Check timed out after 600.0s` — no step of the comparison was reached.

## Steps to Reproduce
1. CreateLaunchTemplate, then CreateAutoScalingGroup with MinSize=0, MaxSize=4, DesiredCapacity=4 and a MixedInstancesPolicy override carrying WeightedCapacity='2'.
2. Poll DescribeAutoScalingGroups for 2 instances with LifecycleState=InService — it never happens.
3. Poll GetMetricStatistics(Namespace='AWS/AutoScaling', MetricName='GroupInServiceCapacity', Dimensions=[{AutoScalingGroupName}]) — no datapoint appears.
