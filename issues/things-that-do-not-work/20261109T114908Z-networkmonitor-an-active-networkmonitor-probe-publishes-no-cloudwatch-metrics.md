Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkmonitor.probe-native-round-trip-metrics
Issue-Shape-Id: com.amazonaws.networkmonitor#NetworkMonitor
Issue-Key: networkmonitor.probe-native-round-trip-metrics|_poll_until#14|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# An ACTIVE networkmonitor probe publishes no CloudWatch metrics: AWS/NetworkMonitor PacketLoss never appears in ListMetrics, so no datapoints are ever readable

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProbe

## Description
Cause 1 of 1: a monitor and probe the emulator drives to ACTIVE never emit anything into CloudWatch, so the metric side of Network Monitor is unobservable. Failing check: networkmonitor.probe-native-round-trip-metrics.

AWS: the networkmonitor Smithy model states the contract in its own docs — CreateMonitor/CreateProbe: 'Within a monitor you'll create one or more probes that monitor network traffic between your source Amazon Web Services VPC subnets and your destination IP addresses. Each probe then aggregates and sends metrics to Amazon CloudWatch', and aggregationPeriod (CreateMonitorInput$aggregationPeriod, @range min 30) is documented as 'The time, in seconds, that metrics are aggregated and sent to Amazon CloudWatch'. On AWS, once the monitor and its probe reach ProbeState ACTIVE, CloudWatch carries metrics in the AWS/NetworkMonitor namespace (RTT, PacketLoss, HealthIndicator) dimensioned by the monitor and the probe, so cloudwatch ListMetrics(Namespace='AWS/NetworkMonitor', MetricName='PacketLoss') lists an entry whose Dimensions include the monitor name and the probeId, and GetMetricStatistics over that metric returns Datapoints at the monitor's aggregation period with SampleCount > 0. For a destination that never replies, every PacketLoss Average is 100.0.

Emulator: the probe reaches ACTIVE (the check's GetProbe poll succeeds) but CloudWatch ListMetrics(Namespace='AWS/NetworkMonitor', MetricName='PacketLoss') returns no entry matching the probeId or monitor name on any page, for the full 600 s poll — the check fails with AssertionError: condition not met within 600.0s; last=None. GetMetricStatistics is never reachable because no metric is ever listed. aggregationPeriod is stored and echoed but has no observable effect.

Why: the workflow the service exists for — create monitor + probe, then read the probe's RTT/PacketLoss in CloudWatch or alarm on it — cannot be exercised. A client that creates a monitor and then polls CloudWatch (or puts a MetricAlarm on AWS/NetworkMonitor PacketLoss) sees an empty namespace, which on AWS only happens when the monitor was never activated, so the client concludes its probe is broken and retries or fails deployment.

Fix: the networkmonitor crate's probe state machine (the component that settles a monitor/probe to ACTIVE) must publish datapoints for the probe into the CloudWatch engine's metric store in the AWS/NetworkMonitor namespace, at the monitor's aggregationPeriod cadence, dimensioned by monitor and probe, with PacketLoss 100 when the destination does not answer — the CloudWatch side (PutMetricData/ListMetrics/GetMetricStatistics) already exists as the propagation target.

Note: open issue 20261024T153105Z-networkmonitor-createprobe-event-propagation.md ('Probes never publish CloudWatch metrics ...') describes this same missing propagation from static inspection, but the replay did not report this check as owned by it; this filing carries the behavioral replay key, so the two should be closed together.

Traces: networkmonitor.probe-native-round-trip-metrics.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against this checkout: networkmonitor 40/41 (98%), 1 failed.

[mismatch] networkmonitor.probe-native-round-trip-metrics — File check_networkmonitor_probe_native_round_trip_metrics.py, line 59 in run: metric_dim = _poll_until(...); line 99 in _poll_until: AssertionError: condition not met within 600.0s; last=None.

The check creates a monitor with aggregationPeriod=30 and one ICMP probe (packetSize 56) from a public subnet it builds, to an unassociated Elastic IP it owns so nothing answers; it asserts the initial state is PENDING, polls GetMonitor until a probe appears, polls GetProbe until state is ACTIVE (it reached ACTIVE), and then polls cloudwatch ListMetrics(Namespace='AWS/NetworkMonitor', MetricName='PacketLoss') across all pages for an entry with a dimension value equal to the probeId or the monitor name. That lookup returned None for 600 s (last=None), so the subsequent GetMetricStatistics assertions — Datapoints non-empty, SampleCount > 0, and the set of Averages == [100.0] for a destination that never replies — were never reached.

Expected: ListMetrics lists the probe's PacketLoss metric dimensioned by the monitor/probe and GetMetricStatistics(Namespace='AWS/NetworkMonitor', MetricName='PacketLoss', Period=60, Statistics=['Average','SampleCount']) returns at least one datapoint with SampleCount > 0 and Average 100.0. Actual: the AWS/NetworkMonitor namespace is empty. Monitor names, probe ids and the Elastic IP are per-run values; the requirement is that an ACTIVE probe produces metrics at all, and that full loss reads 100.

Smithy (networkmonitor): CreateMonitor POST /monitors with aggregationPeriod @range min 30 documented as the interval at which metrics are aggregated and sent to Amazon CloudWatch; CreateProbe POST /monitors/{monitorName}/probes with output state ProbeState.

## Steps to Reproduce
1. CreateMonitor(monitorName='m1', aggregationPeriod=30, probes=[{sourceArn=<public subnet arn>, destination=<an address nothing answers on>, protocol='ICMP', packetSize=56}]) -> 200, state PENDING.
2. Poll GetMonitor('m1') until a probe is listed, then GetProbe(monitorName='m1', probeId=<id>) until state == 'ACTIVE'.
3. Call CloudWatch ListMetrics(Namespace='AWS/NetworkMonitor', MetricName='PacketLoss') and page through it. Expected: an entry whose Dimensions carry the monitor name and the probeId. Actual: no entry at all (the namespace is empty), for at least 600 s.
4. Call GetMetricStatistics(Namespace='AWS/NetworkMonitor', MetricName='PacketLoss', Dimensions=<that metric's dimensions>, Period=60, Statistics=['Average','SampleCount']) over the elapsed window. Expected: datapoints with SampleCount > 0 and Average 100.0 for the unanswered destination. Actual: unreachable, no metric exists.
5. Re-replay networkmonitor.probe-native-round-trip-metrics to confirm.
