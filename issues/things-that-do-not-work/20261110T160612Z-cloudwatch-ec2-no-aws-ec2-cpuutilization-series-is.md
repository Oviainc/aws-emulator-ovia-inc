Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2.detailed-monitoring-shortens-the-cpu-metric-period-and-basic-does-not cloudwatch__ec2.unmonitor-instances-dry-run-rejected-keeps-one-minute-cpu-publication cloudwatch__ec2.unmonitor-instances-restores-five-minute-cpu-publication-after-monitor-instances
Issue-Key: cloudwatch__ec2.detailed-monitoring-shortens-the-cpu-metric-period-and-basic-does-not|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ec2.unmonitor-instances-dry-run-rejected-keeps-one-minute-cpu-publication|_poll#23|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ec2.unmonitor-instances-restores-five-minute-cpu-publication-after-monitor-instances|_poll#22|Timeout|time.sleep(interval_s)
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# cloudwatch__ec2: no AWS/EC2 CPUUtilization series is ever published, so the basic/detailed monitoring cadence and MonitorInstances/UnmonitorInstances have no observable effect

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: a running EC2 instance publishes no AWS/EC2 CPUUtilization datapoints, so neither the basic (5-minute) cadence, nor the detailed (1-minute) cadence after MonitorInstances, nor the restored cadence after UnmonitorInstances can be read.

AWS:      a running instance publishes AWS/EC2 CPUUtilization dimensioned on exactly {InstanceId}, unit Percent, every 5 minutes under basic monitoring and every 1 minute after MonitorInstances ("Basic monitoring | Status check metrics are available in 1-minute periods. All other metrics are available in 5-minute periods" / "Detailed monitoring | You can get metrics in 1-minute periods"); the 5-minute buckets land on wall-clock 5-minute boundaries, so an off-grid timestamp is proof of 1-minute publication. UnmonitorInstances restores the 5-minute cadence, and UnmonitorInstances with DryRun=true changes nothing and is refused with DryRunOperation "Request would have succeeded, but DryRun flag is set." Recorded behavior only for the series content (these metric names are documented, not Smithy shapes).
Emulator: all three checks exhaust their budget with no datapoint to read — `Error: Check timed out after 600.0s` for each. No AWS/EC2 datapoint is ever returned for the launched instances, so not even the positive control (any CPUUtilization datapoint at all under basic monitoring) is established, and the dry-run and restore halves are never reached.
Why:      EC2 is unmonitorable. No CPU alarm can ever fire, no autoscaling or remediation that keys off AWS/EC2 triggers, and the basic-versus-detailed monitoring distinction customers pay for has no effect a client can see.
Fix:      the EC2 instance telemetry component that should emit the AWS/EC2 instance metric families into the CloudWatch metric store, at the cadence selected by the instance's monitoring state, and resume/stop with the instance lifecycle.
Traces:   cloudwatch__ec2.detailed-monitoring-shortens-the-cpu-metric-period-and-basic-does-not, cloudwatch__ec2.unmonitor-instances-dry-run-rejected-keeps-one-minute-cpu-publication, cloudwatch__ec2.unmonitor-instances-restores-five-minute-cpu-publication-after-monitor-instances.

Same root cause as the open issue 20261109T183935Z-cloudwatch-ec2-2-net-new-causes-no-aws.md (Cause 1), which named these three checks under replay keys that no longer match: they now die on a hard 600 s check timeout rather than the recorded AssertionError, so that issue no longer owns them. Please close both together.

## Evidence
Expected (AWS): at least one AWS/EC2 CPUUtilization datapoint per instance within the budget (5-minute grid under basic monitoring); after MonitorInstances a datapoint whose timestamp is NOT on a 5-minute boundary; after UnmonitorInstances only on-grid datapoints again; UnmonitorInstances(DryRun=true) refused with DryRunOperation and the 1-minute publication unchanged.
Actual (emulator), all three checks: `Error: Check timed out after 600.0s`. No AWS/EC2 datapoint was ever read, so the cadence comparison has no positive control.

## Steps to Reproduce
1. RunInstances a t3.micro from the AL2023 AMI and wait for `running`.
2. Poll GetMetricStatistics(Namespace='AWS/EC2', MetricName='CPUUtilization', Dimensions=[{Name:'InstanceId',Value:<id>}], Period=60, Statistics=['Average']) — no datapoint ever appears.
3. MonitorInstances(InstanceIds=[<id>]) and poll again for an off-5-minute-grid datapoint; then UnmonitorInstances and poll for the restored on-grid cadence; also call UnmonitorInstances(DryRun=true) and confirm publication is untouched.
