Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__ec2.ddb-ltr-list-tables-in-a-not-opted-in-region-rejected
Issue-Key: dynamodb__ec2.ddb-ltr-list-tables-in-a-not-opted-in-region-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it")
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# DynamoDB ListTables succeeds in a region the account has not opted into (1 root cause)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
One net-new root cause replayed under the `dynamodb__ec2` suite heading.

Cause 1 of 1: a DynamoDB request addressed to a non-opted-in region is served instead of refused.
AWS: recorded behavior only. A request to a region the account has not opted into is refused before it reaches the service; ListTables has no success path there. An empty TableNames list is the WRONG answer because it is byte-identical to the body an enabled, empty region returns, so the caller cannot distinguish the two.
Emulator: ListTables in that region SUCCEEDS.
Why: region enumeration tools walk every region in the partition and classify each as enabled-and-populated, enabled-and-empty, or not-opted-in. Serving the request collapses the third case into the second, so a client concludes the region is available and proceeds to create tables there - a request AWS will refuse. Any "which regions is this account using?" derivation is wrong in exactly the regions the account cannot use.
Fix: the region opt-in gate in the request-dispatch/auth layer ahead of the DynamoDB handlers (the same gate the EC2 describe paths need).
Traces: dynamodb__ec2.ddb-ltr-list-tables-in-a-not-opted-in-region-rejected

## Evidence
Replayed failing check under `### dynamodb__ec2` (not marked ALREADY FILED / OWNED by):

dynamodb__ec2.ddb-ltr-list-tables-in-a-not-opted-in-region-rejected -> AssertionError: the call succeeded; real AWS rejects it

The companion lifecycle in the same group, dynamodb__ec2.e2e-ddb-ltr-list-tables-all-regions-lifecycle, is OWNED by 20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md and is not claimed here. The EC2 side of the same gap is reported separately for the `ec2` service (ec2.ec2mr-byvpc-describe-instances-in-a-region-not-enabled-rejected, ec2.ec2mr-dvpc-unenumerable-region-rejects-describe-vpcs), which is why this one is filed against its own service heading.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite; the check appears under `### dynamodb__ec2`, unmarked.
2. Standalone repro: build a dynamodb client for a region the emulated account has not opted into (the suite uses the same non-opted-in region list as the EC2 checks, e.g. af-south-1) and call list_tables(). It must be REFUSED, not answered with {'TableNames': []}.
