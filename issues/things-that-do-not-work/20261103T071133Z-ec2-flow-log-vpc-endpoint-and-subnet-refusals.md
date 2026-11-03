Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-vfd-cloudwatch-logs-destination-and-delivery-role
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-vfd-cloudwatch-logs-destination-and-delivery-role ec2-vfd-enum-casing-interval-and-dry-run ec2-vfd-firehose-destination-mistakes ec2-vfd-resource-and-flow-log-id-mistakes-are-atomic ec2-vfd-s3-destination-mistakes-then-recovery ec2-vfd-describe-filter-name-and-token-mistakes ec2-vfd-log-format-field-name-refusals ec2-vvd-traj-subnet-create-mistakes-then-recovery ec2-vpce-traj-iot-data-endpoint-create-refusals
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateFlowLogs|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateFlowLogs|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeFlowLogs|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeFlowLogs|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteFlowLogs|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DeleteFlowLogs|4xx->4xx|$.Response.Errors.Error.Message
Issue-Related: 20261103T071133Z-ec2-invalid-enum-and-attribute-values-are-refused.md
Issue-Related: 20261103T071133Z-ec2-missing-required-parameters-are-refused-as-malformedinput.md
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# EC2 flow-log, VPC-endpoint and subnet refusals answer the wrong error class and authored text

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowLogs

## Description
Six refusals on the flow-log, VPC-endpoint and subnet paths carry the wrong error code or an emulator-authored sentence; two of them also show the emulator's own view of the environment (its AZ list, its field ordering) leaking into the message.

Cause 1 of 6: CreateFlowLogs with an S3 ARN where a CloudWatch Logs destination is required.
AWS: recorded behavior only — 400 Code 'InvalidParameter', Message 'Invalid cloud-watch-logs destination: arn:aws:s3:::<bucket>' (fixed text 'Invalid cloud-watch-logs destination: ' + the LogDestination ARN from the request).
Emulator: 400 Code 'InvalidParameterValue', Message "LogDestination 'arn:aws:s3:::<bucket>' is not the ARN of a CloudWatch Logs log group".
Why: wrong code class plus a reworded sentence; 10 steps across six flow-log fixtures depend on it.
Fix: destination validation in services/service-com-amazonaws-ec2-amazonec2/src/ops/create_flow_logs.rs.
Traces: ec2-vfd-cloudwatch-logs-destination-and-delivery-role#task#6 (cluster also covers ec2-vfd-enum-casing-interval-and-dry-run#task#3..#6, ec2-vfd-firehose-destination-mistakes#task#6, ec2-vfd-resource-and-flow-log-id-mistakes-are-atomic#task#4, ec2-vfd-s3-destination-mistakes-then-recovery#task#4/#5/#6).

Cause 2 of 6: DescribeFlowLogs with an unknown filter name.
AWS: recorded behavior only — 400 Code 'InvalidParameterValue', Message "1 validation error detected: Value 'destination-type' at 'filter.2.member.name' has an invalid format" (fixed text with the request's filter name and its index position substituted).
Emulator: 400 Code 'InvalidFilter', Message 'Filter name must be one of [deliver-log-status, log-destination-type, flow-log-id, log-group-name, resource-id, traffic-type, tag:<key>, tag-key]'.
Why: the codes are swapped relative to the rest of EC2 (here AWS uses InvalidParameterValue), and AWS names the offending filter position while the emulator only lists its own supported names.
Fix: filter validation in describe_flow_logs.rs.
Traces: ec2-vfd-describe-filter-name-and-token-mistakes#task#5 (cluster also covers task#8).

Cause 3 of 6: CreateFlowLogs with unknown LogFormat fields.
AWS: recorded behavior only — 400 Message "Unknown fields provided: 'bar', 'foo'" — the rejected field names are listed in sorted order, each single-quoted, comma-separated.
Emulator: "Unknown fields provided: 'foo', 'bar'" — request order.
Why: the message is not reproducible, so assertions on it are order-dependent; AWS's ordering is part of the fixed form.
Fix: field-name collection/sorting in create_flow_logs.rs log-format validation.
Traces: ec2-vfd-log-format-field-name-refusals#task#5 (cluster also covers task#6).

Cause 4 of 6: DeleteFlowLogs with a malformed flow-log id.
AWS: recorded behavior only — 400 Code 'InvalidParameterValue', Message "1 validation error detected: Value '[fl-bogus]' at 'flowLogId' has an invalid format" (the request's id list rendered in brackets).
Emulator: 400 Code 'InvalidFlowLogId.NotFound', Message 'These flow log ids in the input list are not found: [TotalCount: 1] fl-bogus'.
Why: AWS refuses the id's SYNTAX before any lookup; the emulator reports a lookup miss, so a caller believes the id was well-formed and merely absent.
Fix: id validation in delete_flow_logs.rs.
Traces: ec2-vfd-resource-and-flow-log-id-mistakes-are-atomic#task#10.

Cause 5 of 6: CreateSubnet with an AZ-id where an AZ name is required.
AWS: recorded behavior only — 400 Message 'Value (usw2-az1) for parameter availabilityZone is invalid. Subnets can currently only be created in the following availability zones: us-west-2a, us-west-2b, us-west-2c, us-west-2d.' — fixed text with the request's value and the region's FULL AZ list (lower-case 'availability zones').
Emulator: '... the following Availability Zones: us-west-2a, us-west-2b, us-west-2c.' — capitalised differently and advertising only three AZs, omitting us-west-2d.
Why: the message differs and, worse, it states an AZ set that is not the region's, so a caller retrying with us-west-2d is told it does not exist.
Fix: the region AZ catalog in the EC2 state/catalog plus the refusal text in create_subnet.rs.
Traces: ec2-vvd-traj-subnet-create-mistakes-then-recovery#task#5.

Cause 6 of 6: CreateVpcEndpoint with PrivateDnsEnabled against a VPC lacking DNS attributes.
AWS: recorded behavior only — 400 Message 'Enabling private DNS requires both enableDnsSupport and enableDnsHostnames VPC attributes set to true for vpc-00000000000000024' (fixed text naming both attributes, with the VPC id from the request).
Emulator: "Private DNS can't be enabled because the VPC vpc-00000000000000024 doesn't support DNS hostnames or DNS resolution".
Why: the emulator's sentence does not name the two attributes the caller must set, which is the actionable part of AWS's message.
Fix: refusal text in create_vpc_endpoint.rs.
Traces: ec2-vpce-traj-iot-data-endpoint-create-refusals#task#6.

## Evidence
Replayed wire corpus simparity-wire-aws-v1. Representative: ec2-vfd-cloudwatch-logs-destination-and-delivery-role task#6 — expected 400 / 'InvalidParameter' / 'Invalid cloud-watch-logs destination: arn:aws:s3:::sim2real-probe-vfd-6fb98f2c-absent'; actual 400 / 'InvalidParameterValue' / "LogDestination 'arn:aws:s3:::sim2real-probe-vfd-6fb98f2c-absent' is not the ARN of a CloudWatch Logs log group" (10 steps). Also DescribeFlowLogs (InvalidParameterValue vs InvalidFilter), CreateFlowLogs unknown-field ordering, DeleteFlowLogs (InvalidParameterValue vs InvalidFlowLogId.NotFound), CreateSubnet AZ list (us-west-2d absent) and CreateVpcEndpoint private-DNS wording.

## Steps to Reproduce
1. CreateFlowLogs with LogDestinationType=cloud-watch-logs and an s3 ARN as LogDestination (fixture ec2-vfd-cloudwatch-logs-destination-and-delivery-role, task#6): compare code and message.
2. CreateSubnet with AvailabilityZone=usw2-az1 (ec2-vvd-traj-subnet-create-mistakes-then-recovery, task#5): note the emulator lists three AZs where the recording lists four.
3. Replay the remaining fixture#step tokens in trace_ids and diff code and message.
