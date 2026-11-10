Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__ssm.dcwagent-custom-statsd-value-and-guest-only-memory-metric-reach-cloudwatch
Issue-Key: cloudwatch__ec2__ssm.dcwagent-custom-statsd-value-and-guest-only-memory-metric-reach-cloudwatch|_poll#16|Timeout|time.sleep(interval_s)
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# cloudwatch__ec2__ssm: the SSM-installed CloudWatch agent publishes no custom StatsD or guest memory metric, so the agent check never reaches a verdict

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: metrics produced inside the guest by the CloudWatch agent (a StatsD counter it receives and a memory metric only the guest can measure) never arrive in CloudWatch.

AWS:      an instance with the AmazonSSMManagedInstanceCore profile on which the CloudWatch agent is installed and configured through SSM publishes the configured custom namespace series: a value sent to the agent's StatsD listener appears as a datapoint of the configured metric, and the agent's mem_used_percent (a value no API can derive, only the guest can report) appears in the configured namespace with the configured dimensions. A metric NOT named in the agent configuration never appears. Recorded behavior only (the agent's series are documented behaviour, not Smithy shapes).
Emulator: the check produces no verdict: `Error: Check timed out after 600.0s`. Its waits are the instance becoming an Online SSM managed node, the agent install/configure RunCommand, and GetMetricStatistics returning the agent's series; none completes within the budget.
Why:      guest-sourced telemetry is the only way to monitor memory, disk and application metrics. With none of it arriving, every CloudWatch agent deployment is silently inert: alarms on mem_used_percent or on application StatsD counters stay in INSUFFICIENT_DATA forever.
Fix:      the SSM-managed CloudWatch agent path — the guest agent that should accept StatsD input and publish the configured namespace/dimension series into the CloudWatch metric store (and the instance's SSM registration the install depends on).
Traces:   cloudwatch__ec2__ssm.dcwagent-custom-statsd-value-and-guest-only-memory-metric-reach-cloudwatch.

Same root cause as the open issue 20261107T064927Z-cloudwatch-ec2-ssm-the-ssm-installed-cloudwatch-agent.md, which owns the two sibling agent checks; it does not carry this check's replay key, so this check is unowned. Please close them together.

## Evidence
Expected (AWS): within the budget, GetMetricStatistics over the agent's configured namespace returns the StatsD counter value that was sent to the agent and a mem_used_percent datapoint, while a metric absent from the agent configuration returns no datapoint.
Actual (emulator): `Error: Check timed out after 600.0s` — no agent-published datapoint was ever read.

## Steps to Reproduce
1. RunInstances a t3.micro from the AL2023 AMI with an instance profile carrying arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore.
2. Wait for DescribeInstanceInformation to report it Online, then install/configure the CloudWatch agent through SSM SendCommand with a config declaring a StatsD input and mem_used_percent.
3. Send a StatsD value inside the guest and poll GetMetricStatistics for the configured namespace/metric and for mem_used_percent — nothing is ever published.
