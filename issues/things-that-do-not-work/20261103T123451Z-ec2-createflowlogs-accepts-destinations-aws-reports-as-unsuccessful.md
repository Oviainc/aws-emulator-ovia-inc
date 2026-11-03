Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-vfd-firehose-destination-mistakes
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-vfd-firehose-destination-mistakes ec2-vfd-s3-destination-mistakes-then-recovery ec2-vfd-log-format-field-name-refusals ec2-vfd-cloudwatch-logs-destination-and-delivery-role
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateFlowLogs|2xx->2xx|$.CreateFlowLogsResponse.flowLogIdSet
Issue-Key: ec2|CreateFlowLogs|2xx->2xx|$.CreateFlowLogsResponse.unsuccessful
Issue-Key: ec2|DescribeFlowLogs|2xx->2xx|$.DescribeFlowLogsResponse.flowLogSet
Issue-Key: ec2|DescribeFlowLogs|2xx->2xx|$.DescribeFlowLogsResponse.flowLogSet.item
Issue-Related: 20261030T000411Z-ec2-create-and-describe-responses-omit-modeled-output.md
Issue-Commit: 10d8f420d9792c6a92213c81068536eef787b058

# EC2 CreateFlowLogs accepts destinations AWS reports as unsuccessful

> _Note: this issue was identified against the code base at commit `10d8f420d9792c6a92213c81068536eef787b058`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowLogs

## Description
Three net-new causes with one root: CreateFlowLogs performs no destination-existence check, so it reports success for a destination AWS rejects and the bogus flow logs then pollute every later read.

Cause 1 of 3 — CreateFlowLogs reports success instead of a per-resource failure for a destination that does not exist.
AWS: CreateFlowLogs is a partial-success API. When the log destination does not exist it answers 200 with an EMPTY flowLogIdSet and one unsuccessful item: error.code '400' and error.message 'LogDestination: <destination-name> does not exist' (fixed text, with the destination taken verbatim from the request), resourceId being the monitored resource id from the request. It also echoes clientToken.
Emulator: answers 200 with a created flow-log id in flowLogIdSet, an empty unsuccessful element and no clientToken.
Why: the client believes flow-log delivery is configured when nothing will ever be delivered — the exact failure mode the partial-success contract exists to surface.
Fix: CreateFlowLogs validation (resolve LogDestination before creating) and the unsuccessful/clientToken members of its response serializer.
Traces: ec2-vfd-firehose-destination-mistakes#task#2 (cluster also covers ec2-vfd-s3-destination-mistakes-then-recovery#task#8).

Cause 2 of 3 — the accepted-but-invalid flow logs are then listed by DescribeFlowLogs.
AWS: because the creates failed, the subsequent DescribeFlowLogs returns an EMPTY flowLogSet.
Emulator: returns several ACTIVE flow logs with deliverLogsStatus 'SUCCESS' pointing at the nonexistent Firehose/S3 destinations.
Why: a client auditing flow-log coverage counts delivery it does not have, and deliverLogsStatus 'SUCCESS' actively asserts the opposite of the truth.
Fix: follows from Cause 1 (do not create), plus the DescribeFlowLogs lookup.
Traces: ec2-vfd-firehose-destination-mistakes#task#8 (cluster also covers ec2-vfd-log-format-field-name-refusals#task#8 and ec2-vfd-s3-destination-mistakes-then-recovery#task#9).

Cause 3 of 3 — DescribeFlowLogs returns a duplicate flow log and a destinationOptions default of 'parquet'.
AWS: the fixture's CloudWatch-Logs flow log exists once, so flowLogSet.item is a SINGLE structure; a flow log created without destination options carries fileFormat 'plain-text'.
Emulator: returns a two-element list (an extra flow log from a create AWS refused) and renders destinationOptions.fileFormat 'parquet' as the default.
Why: a client that reads flowLogSet.item as one structure breaks on the list, double-counts configuration, and infers the wrong delivery format.
Fix: the CreateFlowLogs gate above plus the destinationOptions default in the flow-log state/serializer.
Traces: ec2-vfd-cloudwatch-logs-destination-and-delivery-role#task#10.

## Evidence
Cause 1: $.CreateFlowLogsResponse.unsuccessful type_mismatch (expected {'item': {'error': {'code': '400', 'message': 'LogDestination: <name> does not exist'}, 'resourceId': 'vpc-...'}}, got ''); .flowLogIdSet type_mismatch (expected '', got {'item': 'fl-...'}); .clientToken missing.
Cause 2: $.DescribeFlowLogsResponse.flowLogSet type_mismatch (expected '', got a 4-element item list of ACTIVE flow logs with deliverLogsStatus 'SUCCESS' and logDestination arn:aws:firehose:...:deliverystream/<absent-name>).
Cause 3: $.DescribeFlowLogsResponse.flowLogSet.item type_mismatch (expected a single flow log with destinationOptions fileFormat 'plain-text', got a 2-element list whose entries carry destinationOptions {'fileFormat': 'parquet', ...}).

## Steps to Reproduce
Replay the wire corpus (simparity-wire-aws-v1) and inspect ec2-vfd-firehose-destination-mistakes#task#2 and #task#8, and ec2-vfd-cloudwatch-logs-destination-and-delivery-role#task#10. Each fixture issues CreateFlowLogs against a destination name/ARN that does not exist and then re-reads the flow logs.
