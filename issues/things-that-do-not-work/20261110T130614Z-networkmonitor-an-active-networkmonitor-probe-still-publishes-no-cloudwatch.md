Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkmonitor.probe-native-round-trip-metrics
Issue-Shape-Id: com.amazonaws.networkmonitor#NetworkMonitor
Issue-Key: networkmonitor.probe-native-round-trip-metrics|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: 11742957972effc3a277f1c1e061799d0d90da1b

# An ACTIVE networkmonitor probe still publishes no CloudWatch metrics: AWS/NetworkMonitor PacketLoss never appears, and the metrics check now runs to the harness budget

> _Note: this issue was identified against the code base at commit `11742957972effc3a277f1c1e061799d0d90da1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMonitor

## Description
Cause 1 of 1: a monitor whose probe the emulator drives to ACTIVE emits nothing into CloudWatch, so the metric side of Network Monitor is unobservable and the check that reads it never terminates within its budget. Failing check: networkmonitor.probe-native-round-trip-metrics.

Related open issues (same missing propagation, please close together with this one): 20261024T153105Z-networkmonitor-createprobe-event-propagation.md ('Probes never publish CloudWatch metrics ...', found by static inspection) and 20261109T114908Z-networkmonitor-an-active-networkmonitor-probe-publishes-no-cloudwatch-metrics.md (previous behavioral filing of the same check). The replay did NOT report this check as owned by either — its failure signature changed from an inner AssertionError ('condition not met within 600.0s') to a harness-level 'Check timed out after 600.0s' — so it is re-filed here with the current replay key rather than left unreported.

AWS: Smithy (networkmonitor) CreateMonitorInput$aggregationPeriod (@range min 30) is documented as 'The time, in seconds, that metrics are aggregated and sent to Amazon CloudWatch. Valid values are either 30 or 60. 60 is the default if no period is chosen.', and CreateMonitor/CreateProbe docs state 'Within a monitor you'll create one or more probes that monitor network traffic between your source Amazon Web Services VPC subnets and your destination IP addresses. Each probe then aggregates and sends metrics to Amazon CloudWatch'. So once GetProbe reports ProbeState ACTIVE (model enum ProbeState, GetProbe GET /monitors/{monitorName}/probes/{probeId}), CloudWatch carries metrics in the AWS/NetworkMonitor namespace dimensioned by the monitor and the probe: cloudwatch ListMetrics(Namespace='AWS/NetworkMonitor', MetricName='PacketLoss') lists an entry whose Dimensions carry the monitor name and the probeId, and GetMetricStatistics over it returns datapoints at the monitor's aggregation period with SampleCount > 0 — recorded behavior only for the value itself: a destination that never answers loses every packet, so every PacketLoss Average is 100.0.

Emulator: the probe does reach ACTIVE (services/service-com-amazonaws-networkmonitor-networkmonitor/src/state.rs settles PENDING -> ACTIVE after ACTIVATION_AFTER = 500 ms), but nothing is ever published: the crate's state has no reference to the CloudWatch engine, to PutMetricData or to any metric record, so ListMetrics(Namespace='AWS/NetworkMonitor', MetricName='PacketLoss') returns no matching entry on any page. The check's 600 s ListMetrics poll therefore never satisfies its predicate and the check is cut off by the harness budget ('Check timed out after 600.0s'); GetMetricStatistics and the Average == 100.0 assertion are never reached. aggregationPeriod is stored and echoed but has no observable effect anywhere.

Why: the workflow the service exists for — create monitor + probe, then read the probe's RTT/PacketLoss in CloudWatch or alarm on AWS/NetworkMonitor — cannot be exercised. A client that creates a monitor and then polls CloudWatch, or puts a MetricAlarm on AWS/NetworkMonitor PacketLoss, sees an empty namespace, which on AWS only happens when the monitor was never activated; the client concludes its probe is broken and retries or fails its deployment. A client that waits for the first datapoint waits forever.

Fix: the networkmonitor crate's probe state machine — the component that settles a monitor/probe to ACTIVE in its engine state — must publish datapoints for each in-service probe into the CloudWatch engine's metric store, in the AWS/NetworkMonitor namespace, at the monitor's aggregationPeriod cadence, dimensioned by monitor and probe, with PacketLoss 100 for a destination that does not answer. The propagation target already exists on the CloudWatch side (PutMetricData / ListMetrics / GetMetricStatistics are implemented). No status code, error code or message is involved: this is a missing cross-service effect, not a refusal.

Traces: networkmonitor.probe-native-round-trip-metrics.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against this checkout: networkmonitor 40/41 (98%), 1 failed.

[mismatch] networkmonitor.probe-native-round-trip-metrics — Error: Check timed out after 600.0s.

The check creates a monitor with aggregationPeriod=30 and one ICMP probe (packetSize 56) from a public subnet it builds, targeting an unassociated Elastic IP it owns so nothing answers. It asserts the initial state is PENDING, polls GetMonitor until a probe is listed, polls GetProbe until state is ACTIVE, then polls cloudwatch ListMetrics(Namespace='AWS/NetworkMonitor', MetricName='PacketLoss') across all pages (timeout 600 s, interval 20 s) for an entry whose Dimensions contain the probeId or the monitor name. No such entry ever appears, so the poll runs out and the check is cut off at the harness's 600 s budget; the follow-on assertions — GetMetricStatistics Datapoints non-empty, SampleCount > 0, and sorted set of Averages == [100.0] — are never reached.

Expected: ListMetrics lists the probe's PacketLoss metric dimensioned by the monitor/probe, and GetMetricStatistics(Namespace='AWS/NetworkMonitor', MetricName='PacketLoss', Dimensions=<that metric's dimensions>, Period=60, Statistics=['Average','SampleCount']) returns at least one datapoint with SampleCount > 0 and Average 100.0 for the unanswered destination. Actual: the AWS/NetworkMonitor namespace is empty.

Code: services/service-com-amazonaws-networkmonitor-networkmonitor/src/state.rs — ACTIVATION_AFTER (line ~172) and settle_monitor / Probe::settle (lines ~1305, ~1809) flip PENDING -> ACTIVE and run teardown; there is no metric emission or cross-service metric write anywhere in the file.

Monitor names, probe ids, the Elastic IP and timestamps are per-run values and are not part of the requirement; the requirement is that an ACTIVE probe produces metrics at all and that total loss reads 100.

## Steps to Reproduce
1. CreateMonitor(monitorName='m1', aggregationPeriod=30, probes=[{sourceArn=<ARN of a subnet EC2 holds>, destination=<an address nothing answers on>, protocol='ICMP', packetSize=56}]) -> 200, state PENDING.
2. Poll GetMonitor('m1') until a probe is listed, then GetProbe(monitorName='m1', probeId=<id>) until state == 'ACTIVE' (the emulator reaches ACTIVE within ~0.5 s).
3. Call CloudWatch ListMetrics(Namespace='AWS/NetworkMonitor', MetricName='PacketLoss') and page through every page. Expected: an entry whose Dimensions carry the monitor name and the probeId. Actual: no entry at all, for at least 600 s.
4. Call GetMetricStatistics(Namespace='AWS/NetworkMonitor', MetricName='PacketLoss', Dimensions=<that metric's dimensions>, Period=60, Statistics=['Average','SampleCount']) over the elapsed window. Expected: datapoints with SampleCount > 0 and Average 100.0. Actual: unreachable — no metric exists.
5. Re-replay networkmonitor.probe-native-round-trip-metrics to confirm the fix.
