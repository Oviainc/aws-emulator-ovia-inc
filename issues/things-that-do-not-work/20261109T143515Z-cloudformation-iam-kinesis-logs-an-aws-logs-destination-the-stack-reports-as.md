Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__kinesis__logs.cfn-logs-destination-kinesis-round-trip-then-delete cloudformation__iam__kinesis__logs.cfn-logs-destination-malformed-policy-document-rejected
Issue-Key: cloudformation__iam__kinesis__logs.cfn-logs-destination-kinesis-round-trip-then-delete|run#16|AssertionError|assert [d["destinationName"] for d in listed] == [out["DestRef"]], (
Issue-Key: cloudformation__iam__kinesis__logs.cfn-logs-destination-malformed-policy-document-rejected|run#12|AssertionError|assert [d["destinationName"] for d in listed] == [out["DestRef"]], (
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# An AWS::Logs::Destination the stack reports as created is not listed by DescribeDestinations

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the AWS::Logs::Destination resource handler does not create the destination in CloudWatch Logs.
AWS: once a stack declaring AWS::Logs::Destination reaches CREATE_COMPLETE, logs:DescribeDestinations lists exactly one destination whose destinationName is the resource's Ref, with the template's targetArn and roleArn; a malformed DestinationPolicy is refused by the handler and the control destination stays intact (Smithy logs PutDestination / DescribeDestinations).
Emulator: DescribeDestinations returns no entries for the name the stack published — 'DescribeDestinations [] for Ref <destination name>' and 'control DescribeDestinations [] for Ref <destination name>' — so neither the round trip nor the policy-rejection case can be observed.
Why: a client that creates a cross-account log destination by template cannot then put a subscription filter against it: the destination CloudFormation claims to have created does not exist.
Fix: the AWS::Logs::Destination (and its DestinationPolicy) resource handler in the CloudFormation crate — it must call through to the CloudWatch Logs destination state.
Traces: cloudformation__iam__kinesis__logs.cfn-logs-destination-kinesis-round-trip-then-delete, cloudformation__iam__kinesis__logs.cfn-logs-destination-malformed-policy-document-rejected.

## Evidence
Expected: one destination listed for the stack's Ref. Actual: 'AssertionError: DescribeDestinations [] for Ref 'sim2real-cfn-dst-<id>'' and 'AssertionError: control DescribeDestinations [] for Ref 'sim2real-cfn-dstpol-good-<id>''.

## Steps to Reproduce
Replay cloudformation__iam__kinesis__logs.cfn-logs-destination-kinesis-round-trip-then-delete and .cfn-logs-destination-malformed-policy-document-rejected: deploy the stack to CREATE_COMPLETE and call logs:DescribeDestinations for the published Ref.
