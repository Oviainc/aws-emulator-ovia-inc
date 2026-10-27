Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-names-the-group-and-omits-s3-fields
Issue-Key: ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-names-the-group-and-omits-s3-fields|run#11|AssertionError|assert s3_only == [], (
Issue-Commit: ebfe4f059696b431f00c6abb397dd11b94b7073a

# A cloud-watch-logs VPC flow log reports a synthesized S3-shaped LogDestination ARN, so one flow log looks like it has two destinations

> _Note: this issue was identified against the code base at commit `ebfe4f059696b431f00c6abb397dd11b94b7073a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFlowLogs

## Description
Cause 1 of 1: CreateFlowLogs invents a `LogDestination` ARN for a `cloud-watch-logs` flow log that was created with `LogGroupName` only, and DescribeFlowLogs then reports that invented ARN alongside `LogGroupName`.

AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) models `FlowLog.LogDestination` (xmlName `logDestination`, "The Amazon Resource Name (ARN) of the destination for the flow log data") and `FlowLog.DestinationOptions` as OPTIONAL members — neither carries @required — so omitting them from a FlowLog is wire-legal and is what distinguishes the destination shapes. Beyond that the model says nothing about when each member is populated: recorded behavior only. The recording (replayed check, measured against real AWS and restated in the check's own docstring) shows that a flow log created with `LogDestinationType=cloud-watch-logs` and a `LogGroupName` reads back from DescribeFlowLogs with `LogDestinationType=cloud-watch-logs`, the `LogGroupName` and `DeliverLogsPermissionArn` it was given, `MaxAggregationInterval=600` and the default version-2 `LogFormat`, and with NO `LogDestination` member and NO `DestinationOptions` member — those two are the S3/Firehose shape and are populated only for a flow log whose destination was given as an ARN. No error is involved: the call succeeds with HTTP 200 and the divergence is purely in the response representation, so there is no status/error code/message to quote.

Emulator: the response carries an extra `LogDestination` member holding a log-group ARN the request never supplied. Replay reported `a CloudWatch Logs flow log reports S3-shaped keys {'LogDestination': 'arn:aws:logs:<region>:<account-id>:log-group:<log-group-name>'}` — the value is the emulator's own composition of region, account id and the request's `LogGroupName`, not a value from the request (the concrete region/account/group in that one line are per-run values, not part of the requirement; the requirement is that the member is ABSENT). `DestinationOptions` was correctly absent. The synthesis is deliberate in the handler: `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_flow_logs.rs` comments "so whichever member the request used, the flow log reports both" and then does `let destination = input.log_destination.clone().unwrap_or_else(|| state::flow_log_group_arn(&region, &name));`, storing `log_destination: Some(..)` into the `state::FlowLogSpec`. The two-destination appearance therefore originates at create time and is faithfully replayed by DescribeFlowLogs.

Why: a client that reads the destination fields to decide which destination a flow log actually uses — the ordinary way to classify a fleet of flow logs, and exactly what an `ec2 describe-flow-logs` based inventory, a drift detector or an IaC importer does — sees a flow log that declares BOTH a CloudWatch Logs group and a destination ARN. Code that keys off the presence of `LogDestination` (the S3/Firehose discriminator) misroutes the flow log, and a round-trip of the described shape back into CreateFlowLogs now sends `LogDestination` together with `LogGroupName`, a combination real AWS does not require and that no longer reproduces the original request. Absence of a member is also not something a client can defend against: it cannot tell an emulator-invented ARN from one a user really supplied.

Fix: belongs in the CreateFlowLogs handler's destination resolution (`services/service-com-amazonaws-ec2-amazonec2/src/ops/create_flow_logs.rs`) and the `state::FlowLogSpec` it writes — a `cloud-watch-logs` flow log must persist `log_destination` only when the request actually carried `LogDestination`, leaving it `None` when the request named the group via `LogGroupName`; DescribeFlowLogs' serializer must then omit the member rather than defaulting it. The sibling S3/Firehose branch, which legitimately requires `LogDestination`, is unaffected.

Related already-filed issues, neither of which covers this: the open 20261022T235108Z-ec2-createflowlogs-stub.md and the fixed 20261025T034331Z-ec2-flow-log-contract-is-unreachable-the-create.md both describe Create/Describe/DeleteFlowLogs as unimplemented scaffold stubs answering 501. They no longer match the emulator's behavior — the operations now answer and persist state — and this finding is not a refusal, it is a wrong successful response, so it is a net-new defect rather than a reopening of either.

Traces: ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-names-the-group-and-omits-s3-fields.

## Evidence
Check: ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-names-the-group-and-omits-s3-fields (verdict: mismatch).

Expected (real-AWS recording): DescribeFlowLogs on a flow log created as `LogDestinationType=cloud-watch-logs` with `LogGroupName=<group>` and `DeliverLogsPermissionArn=<role>` returns a FlowLog with ResourceId=<vpc>, TrafficType=ALL, LogDestinationType=cloud-watch-logs, LogGroupName=<group>, DeliverLogsPermissionArn=<role>, MaxAggregationInterval=600, LogFormat=the default version-2 format, FlowLogStatus=ACTIVE, DeliverLogsStatus=SUCCESS, and NEITHER a `LogDestination` member NOR a `DestinationOptions` member.

Actual (emulator): the same FlowLog additionally carries `LogDestination`. Replay assertion:

  File "check_ec2__iam__logs_vpc_flow_log_to_cloudwatch_logs_names_the_group_and_omits_s3_fields.py", line 45, in run
    assert s3_only == [], (
  AssertionError: a CloudWatch Logs flow log reports S3-shaped keys {'LogDestination': 'arn:aws:logs:us-west-2:000000000000:log-group:sim2real-fl-VpcFlowLogLogGroup-c4adce4fb253'}

(The region `us-west-2`, account `000000000000` and group suffix in that ARN are per-run values; the requirement is the member's absence, not any particular ARN.)

Source of the synthesized value — services/service-com-amazonaws-ec2-amazonec2/src/ops/create_flow_logs.rs:

  // Where the records go. For CloudWatch Logs the destination is ONE log group named
  // either way ... so whichever member the request used, the flow log reports both
  let destination = input.log_destination.clone()
      .unwrap_or_else(|| state::flow_log_group_arn(&region, &name));
  (Some(name), Some(destination))

and the value is persisted as `state::FlowLogSpec { log_destination: log_destination.clone(), .. }`.

Model evidence: Smithy `com.amazonaws.ec2#FlowLog` members `LogDestination` (String, xmlName logDestination) and `DestinationOptions` (DestinationOptionsResponse, xmlName destinationOptions) are both optional — no @required — so omission is a legal and meaningful wire state.

## Steps to Reproduce
1. Boot the emulator and point an EC2 client at it.
2. CreateVpc (e.g. CidrBlock 10.2.0.0/16) and create an IAM flow-log delivery role; CreateLogGroup in CloudWatch Logs.
3. Call CreateFlowLogs with ResourceType=VPC, ResourceIds=[<vpc-id>], TrafficType=ALL, LogDestinationType=cloud-watch-logs, LogGroupName=<the log group name>, DeliverLogsPermissionArn=<the role ARN> — and deliberately DO NOT pass LogDestination.
4. Call DescribeFlowLogs with FlowLogIds=[<the new flow log id>].
5. Observe the returned FlowLog contains a `LogDestination` member holding `arn:aws:logs:<region>:<account>:log-group:<group>`. Real AWS omits `LogDestination` (and `DestinationOptions`) entirely for a cloud-watch-logs flow log whose destination was named by LogGroupName.
6. Replay check `ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-names-the-group-and-omits-s3-fields` to confirm the fix.
