Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# Probes never publish CloudWatch metrics (AWS/NetworkMonitor namespace is empty after a monitor goes ACTIVE)

_Filed: 2026-10-24T15:31:05Z_
_Operation: CreateProbe | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a CloudWatch Network Monitor probe is a metric producer. The Smithy model's own documentation for CreateMonitor/CreateProbe states the contract explicitly — "Within a monitor you'll create one or more probes that monitor network traffic between your source Amazon Web Services VPC subnets and your destination IP addresses. Each probe then aggregates and sends metrics to Amazon CloudWatch" — and `aggregationPeriod` (30 or 60 seconds) exists only to say how often those datapoints are emitted. On AWS, once a monitor/probe reaches ACTIVE, CloudWatch carries metrics in the `AWS/NetworkMonitor` namespace (RTT/round-trip time, PacketLoss and HealthIndicator) dimensioned by the monitor and the probe, so `cloudwatch:ListMetrics(Namespace="AWS/NetworkMonitor")` returns metric names and `GetMetricStatistics`/`GetMetricData` returns datapoints at the monitor's aggregation period.

Actual: the emulator's networkmonitor crate touches no other service at all. `CreateProbe`/`CreateMonitor` only insert records into this crate's own in-memory store (`crate::state::ComAmazonawsNetworkmonitorNetworkmonitorState`), and `state.rs` has no reference to the CloudWatch engine state, to PutMetricData, or to any metric record. A monitor can therefore sit in state ACTIVE with ACTIVE probes forever while `ListMetrics` against the repo's CloudWatch emulator (which has PutMetricData / ListMetrics / GetMetricStatistics implemented, i.e. the propagation target exists and is observable) reports nothing in `AWS/NetworkMonitor`. `aggregationPeriod` is stored and echoed but has no observable effect anywhere.

Consequence: the end-to-end workflow the service exists for — create monitor + probe, then read the probe's network metrics/alarm on them in CloudWatch — cannot be exercised. Client code that creates a monitor and then polls CloudWatch (or puts a MetricAlarm on AWS/NetworkMonitor RTT/PacketLoss) sees an empty namespace, which on AWS only happens when the monitor was never activated.

## Evidence

Spec: model docs for `CreateMonitor` ("Each probe then aggregates and sends metrics to Amazon CloudWatch") and `CreateMonitorInput$aggregationPeriod` / `MonitorSummary$aggregationPeriod` ("The time, in seconds, that metrics are aggregated and sent to Amazon CloudWatch"); well-established AWS behavior that CloudWatch Network Monitor publishes RTT / PacketLoss / HealthIndicator in the `AWS/NetworkMonitor` namespace.
Handler: services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/create_probe.rs (handle() writes only `store.monitors[...].probes`), src/ops/create_monitor.rs (same), src/state.rs (`settle_monitor` only flips PENDING->ACTIVE; no metric emission, no cross-service engine access anywhere in the crate).
Propagation target exists: services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/{put_metric_data,list_metrics,get_metric_statistics}.rs are implemented (not scaffold stubs).
Tests: no test under services/service-com-amazonaws-networkmonitor-networkmonitor/tests/all/ mentions CloudWatch or metrics, so nothing asserts this today.

## Steps to Reproduce

1. CreateMonitor(monitorName="m1", aggregationPeriod=30, probes=[{sourceArn=<subnet arn>, destination="10.0.0.1", protocol="ICMP", packetSize=56}]) -> 200, state PENDING.
2. Poll GetMonitor("m1") until state is ACTIVE and its probe is ACTIVE.
3. Call CloudWatch ListMetrics(Namespace="AWS/NetworkMonitor"). Expected: metric entries (e.g. RTT / PacketLoss / HealthIndicator) dimensioned by the monitor and probe. Actual: no metrics at all.
4. Call CloudWatch GetMetricStatistics / GetMetricData for the same namespace over the elapsed window. Expected: datapoints at the monitor's 30s aggregation period. Actual: empty.
