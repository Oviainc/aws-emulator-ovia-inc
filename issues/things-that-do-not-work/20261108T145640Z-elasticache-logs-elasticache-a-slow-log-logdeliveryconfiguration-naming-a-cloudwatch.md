Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache__logs.slow-log-delivery-to-missing-log-group-rejected
Issue-Key: elasticache__logs.slow-log-delivery-to-missing-log-group-rejected|run#20|AssertionError|assert bad_cfg is not None and bad_cfg.get("Status") == "error", (
Issue-Commit: 544316519d780db35004270636d68199c524c41e

# elasticache: a slow-log LogDeliveryConfiguration naming a CloudWatch Logs group that does not exist settles to Status 'active' instead of 'error'

> _Note: this issue was identified against the code base at commit `544316519d780db35004270636d68199c524c41e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCacheCluster

## Description
One net-new root cause, reached by 1 check.

Cause 1 of 1: ElastiCache never validates a log-delivery destination asynchronously — a slow-log delivery pointing at a CloudWatch Logs group that does not exist is reported as a healthy, active delivery.

AWS: Smithy (elasticache) LogDeliveryConfiguration carries LogType, DestinationType, DestinationDetails, LogFormat, Status (enum LogDeliveryConfigurationStatus = active|enabling|modifying|disabling|error) and Message, documented as "Returns an error message for the log delivery configuration." Recorded behavior: the request is accepted up front for any destination name, then the configuration settles asynchronously — one naming an existing log group settles to Status 'active' with no Message, and one naming a group that does not exist settles to Status 'error' with Message carrying the fixed text 'Destination log group ' + the requested log group name + ' does not exist.'; ElastiCache never creates the destination it could not find.

Emulator: the configuration for the missing group settles to Status 'active' with no Message: {'LogType': 'slow-log', 'DestinationType': 'cloudwatch-logs', 'DestinationDetails': {'CloudWatchLogsDetails': {'LogGroup': '<missing group>'}}, 'LogFormat': 'json', 'Status': 'active'}. The positive control (an existing group) also settles 'active', so the emulator reports 'active' unconditionally and the 'error' terminal state is unreachable.

Why: 'active' is the client's only signal that logs are actually flowing. A typo'd or not-yet-created destination is indistinguishable from a working one, so an operator (or an IaC readiness check that waits for Status 'active') concludes slow-log delivery is healthy while no log line will ever arrive, and never sees the Message that names the missing group.

Fix: the ElastiCache log-delivery configuration state machine (the component that advances LogDeliveryConfigurations from 'enabling' to a terminal status) — it must resolve the CloudWatch Logs destination, settle to 'error' with that Message when the group does not exist, and must not create the group.

Traces: elasticache__logs.slow-log-delivery-to-missing-log-group-rejected (fails on the settled Status; the Message assertion and the 'the destination group was never created' assertion are unmeasured behind it). This is a different cause from open issue 20261105T090541Z-elasticache-logs-the-slow-log-delivery-check-cannot.md, which reports the Lambda runtime that prevents the in-VPC RESP probe from running in the sibling positive check.

## Evidence
Expected: after CreateCacheCluster with LogDeliveryConfigurations=[{LogType: 'slow-log', DestinationType: 'cloudwatch-logs', DestinationDetails: {CloudWatchLogsDetails: {LogGroup: '<group that does not exist>'}}, LogFormat: 'json', Enabled: true}], the cluster's settled LogDeliveryConfiguration has Status == 'error' and Message == 'Destination log group <group> does not exist.'; DescribeLogGroups(logGroupNamePrefix=<group>) returns [].

Actual (emulator): AssertionError: expected the slow-log delivery naming the missing log group '<group>' to settle to Status='error', got {'LogType': 'slow-log', 'DestinationType': 'cloudwatch-logs', 'DestinationDetails': {'CloudWatchLogsDetails': {'LogGroup': '<group>'}}, 'LogFormat': 'json', 'Status': 'active'} — no Message member at all. The positive control with an existing group settled 'active' as expected, so the two cases are indistinguishable.

## Steps to Reproduce
1. Create a CloudWatch Logs group G and confirm DescribeLogGroups lists it.
2. Create a cache subnet group and pick a security group.
3. Positive control: CreateCacheCluster with LogDeliveryConfigurations naming G (LogType 'slow-log', DestinationType 'cloudwatch-logs', LogFormat 'json', Enabled true); poll DescribeCacheClusters until the LogDeliveryConfiguration leaves 'enabling'. Observe Status 'active'.
4. Pick a log group name M that was never created. CreateCacheCluster again with LogDeliveryConfigurations naming M; poll DescribeCacheClusters until that LogDeliveryConfiguration leaves 'enabling'.
5. Expected: Status 'error' and Message 'Destination log group M does not exist.', and DescribeLogGroups(logGroupNamePrefix=M) still empty.
6. Observed: Status 'active' with no Message.
