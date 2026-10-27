Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-names-the-group-and-omits-s3-fields
Issue-Key: ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-names-the-group-and-omits-s3-fields|run#13|AssertionError|assert status == ("ACTIVE", "SUCCESS"), f"(FlowLogStatus, DeliverLogsStatus) is {status!r}"
Issue-Commit: af5aa10640a9675bf0acdd53eefd768a70c7d565

# EC2 DescribeFlowLogs omits FlowLog.DeliverLogsStatus for a cloud-watch-logs flow log, so delivery health is unreadable and the deliver-log-status filter matches nothing

> _Note: this issue was identified against the code base at commit `af5aa10640a9675bf0acdd53eefd768a70c7d565`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFlowLogs

## Description
One net-new root cause in the EC2 VPC-Flow-Logs surface, reached through CreateFlowLogs and read back through DescribeFlowLogs. Both operations answered 200 and returned a real flow-log record in this replay, so this is a fidelity defect in the implemented path, not a stub refusal.

It is NOT covered by the open issue 20261026T032628Z-ec2-flow-logs-a-cloud-watch-logs-flow.md, nor by the resolved 20261026T031112Z-ec2-a-cloud-watch-logs-vpc-flow-log-reports.md. Those name the synthesized LogDestination ARN (Cause 1) and the missing S3 bucket-policy grants (Cause 2); the LogDestination cause is now fixed, which is precisely why this check progresses past its `s3_only == []` assertion and reaches a LATER, different assertion. The replay key is therefore new (the failure moved from `assert s3_only == []` to `assert status == ("ACTIVE", "SUCCESS")`) and the cause below is distinct from both: it is about a member that is never populated at all, not about one that is wrongly populated.

Cause 1 of 1: a flow log's DeliverLogsStatus is never populated, so DescribeFlowLogs reports no delivery outcome.

AWS:      Smithy (com.amazonaws.ec2#FlowLog) models member `DeliverLogsStatus` (wire name `deliverLogsStatus`, ec2QueryName `DeliverLogsStatus`, target String) documented as "The status of the logs delivery (SUCCESS | FAILED)", alongside `FlowLogStatus` ("The status of the flow log (ACTIVE)") and `DeliverLogsErrorMessage` ("Information about the error that occurred. Rate limited ... Access error indicates that the IAM role associated with the flow log does not have sufficient permissions to publish to CloudWatch Logs ..."). DescribeFlowLogsRequest$Filter documents a `deliver-log-status` filter, "The status of the logs delivery (SUCCESS | FAILED)", which only has anything to select on if the member is present. Recorded behavior (measured on real AWS, asserted by the check): for a flow log created with LogGroupName + DeliverLogsPermissionArn (LogDestinationType cloud-watch-logs), DescribeFlowLogs returns BOTH FlowLogStatus="ACTIVE" and DeliverLogsStatus="SUCCESS"; the two are reported together, and the string values "ACTIVE"/"SUCCESS" are the fixed text of the contract (no part of them is request-derived). When the delivery role is insufficient AWS instead reports DeliverLogsStatus="FAILED" with DeliverLogsErrorMessage="Access error" — i.e. the member is always present on a cloud-watch-logs flow log, carrying one of the two documented values.

Emulator: DescribeFlowLogs answers 200 and returns the flow log with FlowLogStatus="ACTIVE" but with `DeliverLogsStatus` ABSENT. This is deliberate in the checkout, not an oversight of serialization: services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_flow_logs.rs states in its module doc that "no flow log of this emulator carries one — nothing here delivers a flow log RECORD, so there is no delivery outcome to report and the member is left absent rather than claiming SUCCESS", and the `deliver-log-status` arm of `filter_values` carries the same note. CreateFlowLogs (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_flow_logs.rs) never writes a delivery status into the stored record, so the view DescribeFlowLogs clones has nothing to serialize. Replay: AssertionError: (FlowLogStatus, DeliverLogsStatus) is ('ACTIVE', None) — expected ('ACTIVE', 'SUCCESS').

Why:      a client relying on AWS's behavior cannot tell a healthy flow log from a broken one. The documented way to validate a logging setup is to read DeliverLogsStatus (and DeliverLogsErrorMessage on FAILED); against this emulator that read is `None` forever, so a CDK/Terraform/CloudFormation style health gate, a console-equivalent status panel, or any `aws ec2 describe-flow-logs --filter Name=deliver-log-status,Values=SUCCESS` poll either loops to its own timeout or concludes delivery FAILED. Code that does `flow_log["DeliverLogsStatus"]` raises KeyError instead of reading a value, and the documented `deliver-log-status` filter is accepted but matches zero flow logs whatever the store holds, so a filtered listing silently returns an empty set rather than the account's flow logs — a false negative that is worse than a refusal, because the caller cannot distinguish "no flow log delivers" from "this filter is not wired up". Note the emulator's stated rationale (nothing actually delivers a record) does not make absence correct: FlowLogStatus="ACTIVE" is already being reported on the same synthetic basis, so the record claims the flow log is on while refusing to say anything about its delivery.

Fix:      CreateFlowLogs' state projection — the FlowLogSpec-to-FlowLog view in services/service-com-amazonaws-ec2-amazonec2/src/state.rs together with services/service-com-amazonaws-ec2-amazonec2/src/ops/create_flow_logs.rs — must populate `deliver_logs_status` when the flow log is created ("SUCCESS" for the accepted cloud-watch-logs destination, consistently with the "ACTIVE" FlowLogStatus it already reports; "FAILED" plus DeliverLogsErrorMessage where the emulator models an unusable delivery role). services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_flow_logs.rs then needs no change beyond dropping the module-doc and `filter_values` comments that assert the member is intentionally absent — its `deliver-log-status` filter becomes functional as a consequence.

Traces:   ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-names-the-group-and-omits-s3-fields (verdict: mismatch).

## Evidence
Check ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-names-the-group-and-omits-s3-fields [mismatch]:

  File "check_ec2__iam__logs_vpc_flow_log_to_cloudwatch_logs_names_the_group_and_omits_s3_fields.py", line 41, in run
    assert status == ("ACTIVE", "SUCCESS"), f"(FlowLogStatus, DeliverLogsStatus) is {status!r}"
  AssertionError: (FlowLogStatus, DeliverLogsStatus) is ('ACTIVE', None)

Expected (real AWS): DescribeFlowLogs(FlowLogIds=[<id>]) on a cloud-watch-logs flow log returns a FlowLog whose
  FlowLogStatus   == "ACTIVE"    (fixed text, contract value)
  DeliverLogsStatus == "SUCCESS" (fixed text, contract value; "FAILED" with DeliverLogsErrorMessage when the delivery role is insufficient)
All the other members this check asserts now pass: ResourceId=<vpc id from the request>, TrafficType="ALL", LogDestinationType="cloud-watch-logs", LogGroupName=<group from the request>, DeliverLogsPermissionArn=<role ARN from the request>, MaxAggregationInterval=600, LogFormat = the version-2 default field list, and neither LogDestination nor DestinationOptions present.

Actual (emulator): identical record EXCEPT DeliverLogsStatus is absent from the response (botocore surfaces it as a missing key, .get() -> None). HTTP status 200; no error code; nothing is refused.

The omission is asserted as intentional in the checkout:
  services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_flow_logs.rs:10-16  — "`deliver-log-status` compares against `FlowLog$DeliverLogsStatus`, and no flow log of this emulator carries one ... the member is left absent rather than claiming `SUCCESS`."
  services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_flow_logs.rs:427-430 — same note on the `deliver-log-status` arm of `filter_values`.
  services/service-com-amazonaws-ec2-amazonec2/src/ops/create_flow_logs.rs — contains no write of a delivery status (no `deliver_logs_status` / `FlowLogStatus` reference anywhere in the handler).

Second, dependent symptom from the same cause: DescribeFlowLogs(Filters=[{Name: "deliver-log-status", Values: ["SUCCESS"]}]) is accepted (200) and returns FlowLogs=[] for every stored flow log, because filter_values() yields no value to compare.

Smithy citation: com.amazonaws.ec2#FlowLog member DeliverLogsStatus (xmlName deliverLogsStatus, ec2QueryName DeliverLogsStatus, target String) — "The status of the logs delivery (SUCCESS | FAILED)."; member DeliverLogsErrorMessage — "Access error indicates that the IAM role associated with the flow log does not have sufficient permissions to publish to CloudWatch Logs."; DescribeFlowLogsRequest$Filter documents the filter name `deliver-log-status`.

## Steps to Reproduce
1. ec2 CreateVpc(CidrBlock=10.2.0.0/16).
2. iam CreateRole with an AssumeRolePolicyDocument trusting the vpc-flow-logs.amazonaws.com service principal, then PutRolePolicy allowing logs:CreateLogStream, logs:PutLogEvents, logs:DescribeLogGroups, logs:DescribeLogStreams on "*". Keep the role ARN.
3. logs CreateLogGroup(logGroupName=<group>).
4. ec2 CreateFlowLogs(ResourceIds=[<vpc>], ResourceType="VPC", TrafficType="ALL", LogGroupName=<group>, DeliverLogsPermissionArn=<role arn>) — no LogDestination, no LogDestinationType; expect FlowLogIds=[<id>] and Unsuccessful=[].
5. ec2 DescribeFlowLogs(FlowLogIds=[<id>]) and read the single FlowLog record:
     - FlowLogStatus must be "ACTIVE"            -> emulator: "ACTIVE"  (correct)
     - DeliverLogsStatus must be "SUCCESS"       -> emulator: key ABSENT (the divergence)
6. Confirm the knock-on effect: ec2 DescribeFlowLogs(Filters=[{"Name": "deliver-log-status", "Values": ["SUCCESS"]}]) must return the flow log created in step 4; the emulator answers 200 with FlowLogs=[].
