Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudwatch__ec2.weighted-capacity-group-metrics-diverge-from-instance-counts
Issue-Key: autoscaling__cloudwatch__ec2.weighted-capacity-group-metrics-diverge-from-instance-counts|wait#8|Timeout|self._next = min(self._cap, self._next * self._factor)
Issue-Commit: d38d8ab45cd7905c8c6149b13ae22553e33f698d

# autoscaling__cloudwatch__ec2: a weighted-capacity Auto Scaling group never reaches its InService fleet, so the weighted capacity-vs-instance metric families are never published

> _Note: this issue was identified against the code base at commit `d38d8ab45cd7905c8c6149b13ae22553e33f698d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
Cause 1 of 1: a group whose mixed-instances overrides give every instance WeightedCapacity=2 never settles at the instance count its weighted DesiredCapacity implies, so neither the group representation nor the AWS/AutoScaling metric families can be observed.
AWS: recorded behavior only for the convergence itself; the modelled surface backing it is CreateAutoScalingGroup's MixedInstancesPolicy overrides (WeightedCapacity) and EnableMetricsCollection(Metrics, Granularity='1Minute'). The recording shows that with WeightedCapacity=2 on the chosen type and DesiredCapacity=4 (weighted units), the group reaches exactly 2 InService instances, DescribeAutoScalingGroups reports every Instances[*].WeightedCapacity as the string '2' and DesiredCapacity stays 4 (weighted units, not re-expressed as an instance count); after EnableMetricsCollection the group's EnabledMetrics equals exactly the requested subset, and at one common CloudWatch timestamp the CAPACITY family reports weighted units while the INSTANCE family reports instances — GroupInServiceCapacity == WeightedCapacity x GroupInServiceInstances, GroupDesiredCapacity == 4, GroupTotalInstances == 2, GroupTotalCapacity == 4, GroupPendingCapacity == 0 — each with CloudWatch Unit 'None'; ListMetrics indexes exactly the enabled subset and nothing else.
Emulator: the check never gets past its setup poll for the group reaching its InService instance count and is killed by the suite wall clock ("Error: Check timed out after 900.0s"). No InService instance appears, so WeightedCapacity, the weighted DesiredCapacity echo, the enabled-metric subset, the weighted/instance metric split and the ListMetrics index are all unreachable; the poll hangs rather than refusing.
Why: a client that sizes a heterogeneous fleet in weighted capacity units gets a group that never becomes capacity, and its CloudWatch dashboards and alarms over AWS/AutoScaling never receive a datapoint. Because the failure is a hang and not an error, any wait-for-capacity loop blocks indefinitely instead of surfacing a fault.
Fix: the Auto Scaling group reconciliation/launch path that turns DesiredCapacity into Instances (including WeightedCapacity accounting for mixed-instances overrides), and the metric emission that EnableMetricsCollection is supposed to arm for the AWS/AutoScaling namespace. Related but NOT covering this check: open issue 20261107T124656Z-autoscaling-a-group-with-desiredcapacity-0-never-reports-an.md describes a group with DesiredCapacity>0 never reporting an InService instance under the autoscaling token, and 20261104T042502Z-autoscaling-enablemetricscollection-stub.md covers the EnableMetricsCollection scaffold stub; the replay bound this check to neither, so it is carried here.
Traces: autoscaling__cloudwatch__ec2.weighted-capacity-group-metrics-diverge-from-instance-counts.

## Evidence
autoscaling__cloudwatch__ec2.weighted-capacity-group-metrics-diverge-from-instance-counts
  Expected: setup poll "<group> reached 2 InService instances" satisfied within 900s; then Instances[*].WeightedCapacity == ['2','2'], DescribeAutoScalingGroups DesiredCapacity == 4, EnabledMetrics == {GroupDesiredCapacity, GroupInServiceInstances, GroupInServiceCapacity, GroupTotalInstances, GroupTotalCapacity, GroupPendingCapacity}, and at one common timestamp GroupInServiceCapacity == 2 x GroupInServiceInstances (capacity != instances) with Unit 'None' on every point, and ListMetrics indexing exactly that subset.
  Actual: "Error: Check timed out after 900.0s" — the check never advanced past the InService setup poll; no WeightedCapacity, no group metric datapoints and no ListMetrics entries were ever observable.

## Steps to Reproduce
Replay the behavioral suite and re-run autoscaling__cloudwatch__ec2.weighted-capacity-group-metrics-diverge-from-instance-counts:
  1. Resolve an AL2023 AMI, create a VPC/subnets and a launch template for t3.micro.
  2. CreateAutoScalingGroup with MinSize=0, MaxSize=4, DesiredCapacity=4 in WEIGHTED units and a MixedInstancesPolicy whose override gives the instance type WeightedCapacity=2 (so 4 units == 2 instances).
  3. Poll DescribeAutoScalingGroups for 2 Instances in LifecycleState=InService (timeout 900s). Expect 2; observe the poll never satisfied and the check killed at 900s.
  4. (Blocked by step 3) Assert every Instances[*].WeightedCapacity == '2' and DesiredCapacity == 4.
  5. (Blocked) EnableMetricsCollection(AutoScalingGroupName=<group>, Metrics=[GroupDesiredCapacity, GroupInServiceCapacity, GroupInServiceInstances, GroupPendingCapacity, GroupTotalCapacity, GroupTotalInstances], Granularity='1Minute'), then read EnabledMetrics and GetMetricStatistics/ListMetrics in AWS/AutoScaling and assert capacity == 2 x instances at one common timestamp with Unit 'None'.
