Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2.detailed-monitoring-shortens-the-cpu-metric-period-and-basic-does-not cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected cloudwatch__ec2.metrics-ec2-ecs-status-check-metrics-publish-per-minute-while-cpu-stays-five-minute-under-basic-monitoring cloudwatch__ec2.metrics-ec2-ecs-stopped-instance-stops-publishing-but-keeps-its-history-and-aggregates cloudwatch__ec2.unmonitor-instances-dry-run-rejected-keeps-one-minute-cpu-publication cloudwatch__ec2.unmonitor-instances-restores-five-minute-cpu-publication-after-monitor-instances cloudwatch__ec2.alarm-composite-and-actions-ec2-stop-action-disabled-leaves-instance-running
Issue-Key: cloudwatch__ec2.detailed-monitoring-shortens-the-cpu-metric-period-and-basic-does-not|_poll_until#13|AssertionError|raise AssertionError(f"{description} did not happen within {timeout_s}s; last read {last!r}")
Issue-Key: cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ec2.metrics-ec2-ecs-status-check-metrics-publish-per-minute-while-cpu-stays-five-minute-under-basic-monitoring|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ec2.metrics-ec2-ecs-stopped-instance-stops-publishing-but-keeps-its-history-and-aggregates|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ec2.unmonitor-instances-dry-run-rejected-keeps-one-minute-cpu-publication|_poll#23|AssertionError|raise AssertionError(f"{what} did not happen within {budget_s}s; last read {last!r}")
Issue-Key: cloudwatch__ec2.unmonitor-instances-restores-five-minute-cpu-publication-after-monitor-instances|_poll#22|AssertionError|raise AssertionError(f"{what} did not happen within {budget_s}s; last read {last!r}")
Issue-Key: cloudwatch__ec2.alarm-composite-and-actions-ec2-stop-action-disabled-leaves-instance-running|run#32|AssertionError|assert any(f"'{armed_id}'" in h["HistorySummary"] for h in armed_actions), (
Issue-Commit: c33038d6637516191c9271b81207503148b3a194

# cloudwatch__ec2: 2 net-new causes — no AWS/EC2 instance metric series is ever published (CPUUtilization, status checks, monitoring cadence), and an executed ec2:stop alarm action's history item does not name the instance it acted on

> _Note: this issue was identified against the code base at commit `c33038d6637516191c9271b81207503148b3a194`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 2: EC2 instances publish no AWS/EC2 metric series at all, so neither the basic/detailed monitoring cadence, nor the status-check family, nor the stop/terminate lifecycle effects on publication can be observed.
AWS: a running instance publishes AWS/EC2 CPUUtilization dimensioned on {InstanceId} every 5 minutes under basic monitoring and every 1 minute after MonitorInstances (detailed monitoring), while StatusCheckFailed / StatusCheckFailed_Instance / StatusCheckFailed_System publish once a minute regardless of the monitoring level; UnmonitorInstances restores the 5-minute cadence; a stopped instance stops publishing but keeps its history, and a terminated instance cannot be the target of MonitorInstances. Recorded behavior only for the series content (these metric names are documented, not Smithy shapes).
Emulator: no AWS/EC2 datapoint is ever read. The cadence check cannot even establish its positive control ('both basic-monitoring instances publishing at least one AWS/EC2 CPUUtilization datapoint, which is the positive control for the cadence comparison did not happen within 600.0s; last read False'); the MonitorInstances/UnmonitorInstances checks read [] for a 1-minute datapoint; and the terminated-instance, status-check and stopped-instance checks all run out their 900 s ceiling with nothing to read.
Why: EC2 is unmonitorable. No CPU or status-check alarm can ever fire, autoscaling and remediation that key off AWS/EC2 never trigger, and the basic-vs-detailed monitoring distinction (which customers pay for) has no observable effect.
Fix: the EC2 instance telemetry component that should emit the AWS/EC2 instance metric families into the CloudWatch metric store, at the cadence selected by the instance's monitoring state, and stop/resume with the instance lifecycle.
Failing checks: cloudwatch__ec2.detailed-monitoring-shortens-the-cpu-metric-period-and-basic-does-not, cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected, cloudwatch__ec2.metrics-ec2-ecs-status-check-metrics-publish-per-minute-while-cpu-stays-five-minute-under-basic-monitoring, cloudwatch__ec2.metrics-ec2-ecs-stopped-instance-stops-publishing-but-keeps-its-history-and-aggregates, cloudwatch__ec2.unmonitor-instances-dry-run-rejected-keeps-one-minute-cpu-publication, cloudwatch__ec2.unmonitor-instances-restores-five-minute-cpu-publication-after-monitor-instances.

Cause 2 of 2: the alarm-history Action item for an executed ec2:stop automate action does not name the instance it acted on.
AWS: when an alarm executes an 'arn:aws:automate:<region>:ec2:stop' action, the Action item CloudWatch writes into DescribeAlarmHistory names the instance the action was applied to as well as the action ARN, so the history is an audit record of what was stopped. Recorded behavior only (the history summary text is not modelled in Smithy); the requirement is that the item identifies the target instance id, not the literal id from any one run.
Emulator: the only Action history item is 'Successfully executed action arn:aws:automate:us-west-2:ec2:stop' — the instance id is absent, and the disabled twin's untouched state cannot be attributed.
Why: alarm history is the audit trail for automated remediation. Without the target in the record a customer cannot tell which instance an alarm stopped, so automated stop/reboot actions are unauditable.
Fix: the alarm action-dispatch component's history writer for EC2 automate actions — include the resolved target instance id in the Action item summary.
Failing check: cloudwatch__ec2.alarm-composite-and-actions-ec2-stop-action-disabled-leaves-instance-running.

## Evidence
Cause 1 — Expected: AWS/EC2 CPUUtilization datapoints for each instance (5-minute grid under basic monitoring, 1-minute after MonitorInstances) and per-minute StatusCheckFailed* datapoints. Actual: 'AssertionError: both basic-monitoring instances publishing at least one AWS/EC2 CPUUtilization datapoint ... did not happen within 600.0s; last read False'; 'AssertionError: a 1-minute (off 5-minute grid) CPUUtilization datapoint for i-0000000000000000 1 after MonitorInstances did not happen within 600.0s; last read []'; three further checks killed by 'Error: Check timed out after 900.0s'.
Cause 2 — Expected: an Action history item naming the instance the ec2:stop action was executed against. Actual: 'AssertionError: no Action history item names instance \'i-00000000000000001\': [\'Successfully executed action arn:aws:automate:us-west-2:ec2:stop\']' (the instance id is per-run; the missing field is the divergence).

## Steps to Reproduce
Cause 1: RunInstances two instances; poll GetMetricStatistics(Namespace='AWS/EC2', MetricName='CPUUtilization', Dimensions=[{InstanceId}], Period=60) — nothing appears. Call MonitorInstances and poll again for an off-5-minute-grid datapoint; call UnmonitorInstances and poll for the restored cadence; poll StatusCheckFailed_Instance per minute.
Cause 2: PutMetricAlarm with AlarmActions=['arn:aws:automate:<region>:ec2:stop'] on an instance's metric, drive it to ALARM, then read DescribeAlarmHistory(HistoryItemType='Action') and inspect the item's summary for the instance id.
