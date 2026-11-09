Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kinesis__logs__sts.put-subscription-filter-cross-region-destination-rejected
Issue-Key: iam__kinesis__logs__sts.put-subscription-filter-cross-region-destination-rejected|_error#28|AssertionError|raise AssertionError(
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# PutSubscriptionFilter accepts a destination in another region

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
Cause 1 of 1: the cross-region destination check is missing, so an undeliverable filter is created.
AWS: a subscription filter's destinationArn must be in the same region as the log group; a Kinesis stream ARN in another region is refused with InvalidParameterException (HTTP 400) and no filter is created (Smithy logs PutSubscriptionFilter errors: InvalidParameterException, ResourceNotFoundException, OperationAbortedException, LimitExceededException, ServiceUnavailableException).
Emulator: the call returns HTTP 200 with an empty body — 'expected the call to be rejected, but it SUCCEEDED and returned {... 'HTTPStatusCode': 200 ...}' — so a filter pointing at an unreachable destination is stored.
Why: a client gets a filter that silently delivers nothing instead of an immediate validation error naming the mistake.
Fix: destination validation in the PutSubscriptionFilter handler of the CloudWatch Logs crate (region of the destination ARN must match the log group's region).
Traces: iam__kinesis__logs__sts.put-subscription-filter-cross-region-destination-rejected.

## Evidence
Expected: 400 InvalidParameterException. Actual: 'AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}'.

## Steps to Reproduce
Replay iam__kinesis__logs__sts.put-subscription-filter-cross-region-destination-rejected: call PutSubscriptionFilter with a Kinesis destination ARN whose region differs from the log group's.
