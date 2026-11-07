Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__ssm.dcwagent-disk-used-percent-tracks-bytes-written-in-guest-while-an-unconfigured-metric-never-appears cloudwatch__ec2__ssm.dcwagent-statsd-value-blocked-while-the-agent-is-not-running-then-arrives-once-started
Issue-Key: cloudwatch__ec2__ssm.dcwagent-disk-used-percent-tracks-bytes-written-in-guest-while-an-unconfigured-metric-never-appears|_poll#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ec2__ssm.dcwagent-statsd-value-blocked-while-the-agent-is-not-running-then-arrives-once-started|_poll#17|Timeout|time.sleep(interval_s)
Issue-Commit: 67bb1f0baca61c1b5b95e0062b2898063e06b421

# cloudwatch__ec2__ssm: the SSM-installed CloudWatch agent never publishes guest metrics, so both agent checks reach no verdict

> _Note: this issue was identified against the code base at commit `67bb1f0baca61c1b5b95e0062b2898063e06b421`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricData

## Description
Cause 1 of 1: a CloudWatch-agent configuration pushed to an EC2 instance through SSM never results in any agent-published series in CloudWatch, so neither the configured-metric arm nor the agent-not-running arm of these checks ever reaches its observable and both exhaust the harness budget.

AWS: an SSM-managed EC2 instance whose role carries AmazonSSMManagedInstanceCore and CloudWatchAgentServerPolicy, configured with an agent config and started via `amazon-cloudwatch-agent-ctl`, publishes the metrics its config declares (for example `disk_used_percent` under the configured namespace with the configured dimensions) and publishes nothing for a metric the config does not declare — an undeclared metric name simply never appears in ListMetrics/GetMetricStatistics. Likewise a StatsD value sent inside the guest is only accepted into CloudWatch while the agent is running: with the agent stopped the value is dropped and no datapoint appears, and after the agent is started the same value arrives as a datapoint. Smithy (cloudwatch): PutMetricData/ListMetrics/GetMetricStatistics carry no agent-specific contract — this is recorded behavior only; the agent is an in-guest publisher that reaches CloudWatch through PutMetricData.

Emulator: both checks are reported `mismatch` with `Error: Check timed out after 60.0s`. No agent-published datapoint ever appears for the declared metric, so the positive arm never settles; and because the declared series never appears, the negative arms (the undeclared metric that must never appear, and the StatsD value that must be blocked while the agent is stopped) are vacuous — they cannot distinguish a working suppression from a publisher that never runs at all.

Why: a client that relies on host-level CloudWatch agent metrics (disk, memory, StatsD custom metrics) gets no series at all, and a test that asserts an unconfigured metric is absent passes for the wrong reason. Alarms and dashboards built on agent metrics are silently empty.

Fix: the EC2 guest/SSM RunCommand execution path (the component that must actually run the pushed document in the instance) and the agent's publish path into the cloudwatch metric store: an agent configuration applied through SSM must produce datapoints for exactly the declared metrics while the agent is running, and none while it is stopped.

Traces: the two failing checks listed under `### cloudwatch__ec2__ssm`, both timing out at the 60s cap.

## Evidence
Expected: (a) with the agent configured for `disk_used_percent` and running, GetMetricStatistics on that metric under the configured namespace/dimensions returns datapoints that track bytes written in the guest, while a metric name the config never declared returns no datapoints ever; (b) a StatsD value sent in the guest while the agent is stopped produces no datapoint, and the same value sent after the agent is started produces one.

Actual: both checks are reported `[mismatch] ... Error: Check timed out after 60.0s`. No agent-published datapoint of any kind is observed, so neither the positive arm nor the negative arm of either check reaches its assertion.

## Steps to Reproduce
1. Launch an Amazon Linux 2023 t3.micro in a dedicated VPC/subnet with an instance profile carrying AmazonSSMManagedInstanceCore and CloudWatchAgentServerPolicy; wait for it to register with SSM.
2. Push a CloudWatch agent configuration through SSM (SendCommand) declaring `disk_used_percent` (and a StatsD listener for the second check) and start the agent with `amazon-cloudwatch-agent-ctl`.
3. Write bytes in the guest / send a StatsD value in the guest.
4. Poll ListMetrics/GetMetricStatistics for the declared metric (and for a metric the config never declared).
5. Observe that no datapoint ever appears for the declared metric; the checks run to the 60s harness cap.
