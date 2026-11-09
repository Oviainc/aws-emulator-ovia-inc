Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__elbv2.albos-alb-publishes-unhealthy-host-count-for-routed-stale-targets-and-none-for-an-unrouted-twin cloudwatch__ec2__elbv2.albos-unhealthy-host-alarm-fires-from-published-datapoints-under-not-breaching cloudwatch__ec2__elbv2.albos-cli-solve-queries-find-the-alb-by-dns-name-and-its-stale-targets
Issue-Key: cloudwatch__ec2__elbv2.albos-alb-publishes-unhealthy-host-count-for-routed-stale-targets-and-none-for-an-unrouted-twin|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ec2__elbv2.albos-unhealthy-host-alarm-fires-from-published-datapoints-under-not-breaching|_poll#7|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ec2__elbv2.albos-cli-solve-queries-find-the-alb-by-dns-name-and-its-stale-targets|_poll#17|Timeout|time.sleep(interval_s)
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# cloudwatch__ec2__elbv2: 2 net-new root causes — an ALB publishes no AWS/ApplicationELB host-count metrics (so the derived alarm never fires), and its ENIs never appear in the VPC

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Two net-new root causes behind the three failing cloudwatch__ec2__elbv2 checks. All three end as 'Check timed out after 60.0s'; in each case the only unbounded wait reached is a poll whose predicate the emulator can never satisfy — the emulator-side cause is named per cause below, and the per-cause grounding is reinforced by the independently confirmed ec2__elbv2 finding that routed IP targets read 'healthy' forever.

Cause 1 of 2: no AWS/ApplicationELB UnHealthyHostCount/HealthyHostCount series is ever published for an ALB's target groups.
AWS: an ALB publishes per-minute UnHealthyHostCount and HealthyHostCount into namespace AWS/ApplicationELB under the dimension pair TargetGroup=targetgroup/<name>/<id> and LoadBalancer=app/<name>/<id> for every target group a listener routes to — with two unreachable registered targets, UnHealthyHostCount reaches 2 and HealthyHostCount reads 0 — and publishes NOTHING under that LoadBalancer dimension for a target group no listener references. A metric alarm on that series with Statistic Maximum, Period 60, EvaluationPeriods 2, Threshold 0, ComparisonOperator GreaterThanThreshold therefore reaches StateValue 'ALARM' from real datapoints, with StateReasonData.recentDatapoints of 2.0, threshold 0.0 and statistic 'Maximum' — even under TreatMissingData 'notBreaching', where silence cannot fire it. Recorded behavior only (these are service-published metrics, not modeled shapes).
Emulator: GetMetricStatistics for those dimensions never yields the datapoints, so both checks spin until the suite's 60 s ceiling: the UnHealthyHostCount poll (budget 600 s) never sees 2.0 and the alarm poll (budget 600 s) never leaves INSUFFICIENT_DATA/OK. Consistent with the confirmed defect that the emulator marks registered targets healthy and never grades them: there is no unhealthy host to count. Hypothesis: the emulator has no ALB metric publisher at all, so even HealthyHostCount=0 is absent.
Why: every ALB observability workflow — dashboards, unhealthy-host alarms, auto-remediation driven by them — is silent, and an alarm configured exactly as AWS documents never fires.
Fix: the ELBv2 load-balancer/target-group health evaluation must publish the AWS/ApplicationELB host-count series into CloudWatch metric state, and CloudWatch alarm evaluation must grade it (never set_alarm_state).
Traces: cloudwatch__ec2__elbv2.albos-alb-publishes-unhealthy-host-count-for-routed-stale-targets-and-none-for-an-unrouted-twin, cloudwatch__ec2__elbv2.albos-unhealthy-host-alarm-fires-from-published-datapoints-under-not-breaching.

Cause 2 of 2: an ALB's own network interfaces never appear in its VPC.
AWS: creating an ALB attaches one ENI per subnet, so EC2 DescribeNetworkInterfaces filtered Name=vpc-id,Values=<the ALB's VPC> lists at least two interfaces with private addresses in the ALB's subnets (the stack's own ENIs are how a diagnosis attributes addresses to the load balancer). Recorded behavior only.
Emulator: the poll for at least two interfaces in the VPC (budget 600 s) never satisfies and the check hits the 60 s ceiling, so the ALB appears to own no ENI. The same check's earlier assertions (DescribeLoadBalancers by DNSName, VpcId, the ALB's two target groups and their registered ids) are all reached through the aws CLI before the poll, so the CLI itself was available and answering.
Why: any address-level diagnosis (which ENI is the ALB, which addresses are stale targets) cannot be performed; the ALB is invisible in EC2.
Fix: ALB provisioning must materialise its ENIs in EC2 network-interface state (and release them on delete).
Traces: cloudwatch__ec2__elbv2.albos-cli-solve-queries-find-the-alb-by-dns-name-and-its-stale-targets.

## Evidence
Cause 1: expected GetMetricStatistics(Namespace='AWS/ApplicationELB', MetricName='UnHealthyHostCount', Period=60, Statistics=['Maximum'], Dimensions=[TargetGroup=<tg suffix>, LoadBalancer=<lb suffix>]) to yield Maximum 2.0 for both routed groups and HealthyHostCount 0.0, and the twin's series to be []; and expected DescribeAlarms StateValue 'ALARM' with StateReasonData recentDatapoints all 2.0, threshold 0.0, statistic 'Maximum'. Got: no datapoints at all — both checks end 'Check timed out after 60.0s' with their polls (600 s budgets, 15 s interval) unsatisfied.
Cause 2: expected DescribeNetworkInterfaces --filters Name=vpc-id,Values=<vpc> to list >= 2 private addresses; got an unsatisfied poll, 'Check timed out after 60.0s'. The CLI-driven DescribeLoadBalancers/DescribeTargetGroups/DescribeTargetHealth assertions that precede it did not fail.

## Steps to Reproduce
Replay the behavioral suite and re-run the cloudwatch__ec2__elbv2 checks in trace_ids. Build the estate the checks build: an internal ALB in two private subnets, two ip target groups each registering two unreachable in-VPC addresses on port 443, a listener forwarding to the first and a priority-10 path rule forwarding to the second, plus an unrouted twin group holding the same addresses. Then (1) poll GetMetricStatistics for AWS/ApplicationELB UnHealthyHostCount/HealthyHostCount under the TargetGroup+LoadBalancer dimension pair, and PutMetricAlarm on UnHealthyHostCount (Maximum, Period 60, EvaluationPeriods 2, Threshold 0, GreaterThanThreshold, TreatMissingData notBreaching) and poll DescribeAlarms for ALARM; (2) poll EC2 DescribeNetworkInterfaces filtered on the ALB's vpc-id for its two ENIs.
