Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__s3.describe-flow-logs-with-an-unknown-filter-name-rejected ec2__s3.duplicate-vpc-flow-log-to-the-same-bucket-rejected ec2__s3.subnet-id-under-the-vpc-resource-type-rejected ec2__s3.vpc-flow-log-naming-both-a-log-group-and-a-bucket-rejected ec2__s3.vpc-flow-log-to-a-missing-bucket-rejected-per-resource ec2__s3.vpc-flow-log-to-s3-applies-documented-defaults ec2__s3.vpc-flow-log-with-a-300-second-aggregation-interval-rejected
Issue-Key: ec2__s3.describe-flow-logs-with-an-unknown-filter-name-rejected|run#10|AssertionError|assert got == want, f"an unknown filter name returned {got!r}, expected {want!r}"
Issue-Key: ec2__s3.duplicate-vpc-flow-log-to-the-same-bucket-rejected|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Key: ec2__s3.subnet-id-under-the-vpc-resource-type-rejected|run#8|AssertionError|assert code == "InvalidParameter", f"a subnet id under ResourceType=VPC was refused {code!r}"
Issue-Key: ec2__s3.vpc-flow-log-naming-both-a-log-group-and-a-bucket-rejected|run#6|AssertionError|assert code == "InvalidParameter", f"LogGroupName + LogDestination was refused {code!r}"
Issue-Key: ec2__s3.vpc-flow-log-to-a-missing-bucket-rejected-per-resource|run#6|AssertionError|assert ids == [], f"a flow log to a missing bucket got ids {ids!r}"
Issue-Key: ec2__s3.vpc-flow-log-to-s3-applies-documented-defaults|run#12|AssertionError|assert f.get("DestinationOptions") == {"FileFormat": "plain-text",
Issue-Key: ec2__s3.vpc-flow-log-with-a-300-second-aggregation-interval-rejected|run#9|AssertionError|assert (code, message) == expected, (
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# VPC flow logs: unknown-filter and parameter refusals use the wrong classes, duplicates and missing buckets are accepted, and documented DestinationOptions defaults are absent (7 root causes)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowLogs

## Description
Seven net-new root causes in the EC2 flow-logs surface, replayed under the `ec2__s3` suite heading. Smithy: CreateFlowLogs (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) requires ResourceIds + ResourceType, accepts LogGroupName, LogDestination, LogDestinationType, MaxAggregationInterval and DestinationOptions, returns FlowLogIds and an Unsuccessful list, and declares "errors": [] (ec2Query puts refusals out of band) - so every code/message below is the recorded AWS behavior.

Cause 1 of 7: DescribeFlowLogs refuses an unknown filter name with the wrong code and an emulator-authored message.
AWS: an unknown filter NAME is refused InvalidParameterValue with the generic input-validation text `1 validation error detected: Value '<name>' at 'filter.1.member.name' has an invalid format` - fixed text around the submitted name and the member path (the index follows the position in the request).
Emulator: answers InvalidFilter with `Filter name must be one of [deliver-log-status, log-destination-type, flow-log-id, log-group-name, resource-id, traffic-type, tag:<key>, tag-key]` - a code AWS does not use here plus the emulator's own enumeration.
Why: a caller branching on InvalidParameterValue (bad input, do not retry) never matches InvalidFilter, and the member path AWS supplies - the only machine-usable pointer to which filter was wrong - is missing.
Fix: the filter-name validation in the DescribeFlowLogs handler.
Traces: ec2__s3.describe-flow-logs-with-an-unknown-filter-name-rejected

Cause 2 of 7: a duplicate flow log to the same destination is accepted.
AWS: recorded behavior only. A second flow log for the same resource with the same traffic type and the same S3 destination is refused; there is no success path.
Emulator: the call SUCCEEDS.
Why: AWS's uniqueness rule is what stops double-billed, double-written log delivery; a client that relies on the refusal to make its provisioning idempotent instead creates duplicates.
Fix: the per-(resource, traffic type, destination) uniqueness check in the CreateFlowLogs handler.
Traces: ec2__s3.duplicate-vpc-flow-log-to-the-same-bucket-rejected

Cause 3 of 7: a subnet id under ResourceType=VPC is refused with the wrong class.
AWS: recorded behavior only. Passing a subnet id while ResourceType='VPC' is a resource-type mismatch, refused with AWS's own class for that condition - not a VPC-not-found.
Emulator: answers InvalidVpcId.NotFound, asserting the VPC does not exist when the caller actually mismatched the type.
Why: the caller is told to create the VPC rather than to fix ResourceType, so the request can never be repaired by following the error.
Fix: the ResourceType/ResourceIds consistency check in the CreateFlowLogs handler, ahead of resource lookup.
Traces: ec2__s3.subnet-id-under-the-vpc-resource-type-rejected

Cause 4 of 7: naming both LogGroupName and LogDestination is refused with the wrong class.
AWS: recorded behavior only. Supplying both a CloudWatch Logs group and an S3 destination in one CreateFlowLogs is refused with AWS's own class for that conflict, not InvalidParameterCombination.
Emulator: answers InvalidParameterCombination.
Why: clients that branch on AWS's class to drop one destination and retry never reach that branch.
Fix: the destination-exclusivity validation in the CreateFlowLogs handler.
Traces: ec2__s3.vpc-flow-log-naming-both-a-log-group-and-a-bucket-rejected

Cause 5 of 7: a flow log to a nonexistent bucket is created instead of being reported per-resource.
AWS: recorded behavior only. CreateFlowLogs to a missing S3 bucket returns the failure in the per-resource `Unsuccessful` list and mints NO flow-log id for it.
Emulator: returns a FlowLogIds entry (`['fl-...']`) and no Unsuccessful entry.
Why: CreateFlowLogs is a partial-success API; a client reads Unsuccessful to learn which resources failed. The emulator reports complete success, so the caller believes logs are being delivered to a bucket that does not exist.
Fix: destination validation plus the Unsuccessful/FlowLogIds split in the CreateFlowLogs handler.
Traces: ec2__s3.vpc-flow-log-to-a-missing-bucket-rejected-per-resource

Cause 6 of 7: DestinationOptions defaults are absent from the created flow log.
AWS: recorded behavior only. An S3 flow log created without DestinationOptions reads back with the documented defaults populated (file format, Hive-compatible partitions and per-hour partitioning flags); the member is present, not null.
Emulator: DescribeFlowLogs reports DestinationOptions as None.
Why: a client that reads DestinationOptions to decide how to parse the delivered objects (plain text vs parquet, Hive partition layout) gets nothing and cannot locate or decode the logs.
Fix: default application in the CreateFlowLogs handler plus the DescribeFlowLogs serializer.
Traces: ec2__s3.vpc-flow-log-to-s3-applies-documented-defaults

Cause 7 of 7: an invalid MaxAggregationInterval is refused with the wrong code and an emulator-authored message.
AWS: recorded behavior only. MaxAggregationInterval=300 is refused ('InvalidParameter', 'Invalid Flow Log Max Aggregation Interval.') - that fixed sentence, with no variable parts.
Emulator: answers ('InvalidParameterValue', 'MaxAggregationInterval must be one of [60, 600], not 300'), inventing both the code and the text.
Why: both the code a client branches on and the message it surfaces diverge, so neither automated handling nor an operator reading the log sees what AWS reports.
Fix: the MaxAggregationInterval validation in the CreateFlowLogs handler.
Traces: ec2__s3.vpc-flow-log-with-a-300-second-aggregation-interval-rejected

## Evidence
Replayed failing checks under `### ec2__s3` (none marked ALREADY FILED / OWNED by):

1. ec2__s3.describe-flow-logs-with-an-unknown-filter-name-rejected -> AssertionError: an unknown filter name returned ('InvalidFilter', 'Filter name must be one of [deliver-log-status, log-destination-type, flow-log-id, log-group-name, resource-id, traffic-type, tag:<key>, tag-key]'), expected ('InvalidParameterValue', "1 validation error detected: Value 'destination-bucket' at 'filter.1.member.name' has an invalid format")
2. ec2__s3.duplicate-vpc-flow-log-to-the-same-bucket-rejected -> AssertionError: the call succeeded; real AWS rejects it, so there is no error
3. ec2__s3.subnet-id-under-the-vpc-resource-type-rejected -> AssertionError: a subnet id under ResourceType=VPC was refused 'InvalidVpcId.NotFound'
4. ec2__s3.vpc-flow-log-naming-both-a-log-group-and-a-bucket-rejected -> AssertionError: LogGroupName + LogDestination was refused 'InvalidParameterCombination'
5. ec2__s3.vpc-flow-log-to-a-missing-bucket-rejected-per-resource -> AssertionError: a flow log to a missing bucket got ids ['fl-0000000000000002b']
6. ec2__s3.vpc-flow-log-to-s3-applies-documented-defaults -> AssertionError: flow log on 'vpc-00000000000000024': DestinationOptions defaults read back None
7. ec2__s3.vpc-flow-log-with-a-300-second-aggregation-interval-rejected -> AssertionError: MaxAggregationInterval=300 was refused ('InvalidParameterValue', 'MaxAggregationInterval must be one of [60, 600], not 300'), expected ('InvalidParameter', 'Invalid Flow Log Max Aggregation Interval.')

Model evidence: smithy_query(query="operation", operation="CreateFlowLogs") -> input {ResourceIds (required, list), ResourceType (required, enum), LogGroupName, LogDestination, LogDestinationType, MaxAggregationInterval, DestinationOptions, ...}; output {ClientToken, FlowLogIds, Unsuccessful}; errors: []. The Unsuccessful member is the partial-failure channel cause 5 relies on.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite; the seven checks appear under `### ec2__s3`, unmarked.
2. Standalone repros (one VPC and one S3 bucket needed):
   - DescribeFlowLogs(Filter=[{Name:'destination-bucket',Values:['x']}]): expect InvalidParameterValue with `1 validation error detected: Value 'destination-bucket' at 'filter.1.member.name' has an invalid format`.
   - CreateFlowLogs(ResourceType='VPC', ResourceIds=[vpc], TrafficType='ALL', LogDestinationType='s3', LogDestination=<bucket arn>) twice: the second must be refused.
   - CreateFlowLogs(ResourceType='VPC', ResourceIds=[<a subnet id>]): expect a resource-type mismatch refusal, not InvalidVpcId.NotFound.
   - CreateFlowLogs with both LogGroupName and LogDestination set: expect AWS's conflict class.
   - CreateFlowLogs to `arn:aws:s3:::<bucket-that-does-not-exist>`: FlowLogIds must be empty and Unsuccessful must carry the resource.
   - CreateFlowLogs to s3 without DestinationOptions, then DescribeFlowLogs: DestinationOptions must be populated with the documented defaults.
   - CreateFlowLogs with MaxAggregationInterval=300: expect ('InvalidParameter', 'Invalid Flow Log Max Aggregation Interval.').
