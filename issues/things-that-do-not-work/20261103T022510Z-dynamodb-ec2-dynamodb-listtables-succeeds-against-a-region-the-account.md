Issue-Severity: MEDIUM
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# DynamoDB ListTables succeeds against a region the account is not opted in to

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
One net-new root cause.

Cause 1 of 1: a DynamoDB call addressed to a region the account has not opted in to is served instead of refused.
AWS: regional opt-in is enforced at the endpoint. A DynamoDB request sent to a non-opted-in region (the set of opt-in-required regions is read from EC2 DescribeRegions) does not reach ListTables at all — the call is refused, so the caller never receives a table list for that region. Recorded behavior only: the check captures the refusal AWS serves and asserts a refusal occurred; the emulator returns none, so there is no error code to compare against.
Emulator: ListTables against the non-opted-in region succeeds and returns a table list.
Why: a client enumerating tables across regions (a backup sweep, an inventory tool, a tag reconciler) believes it has coverage in regions the account cannot actually use, and code that treats the refusal as "skip this region" never takes that branch. Against real AWS the same sweep fails on the region the emulator happily served.
Fix: the regional opt-in gate in front of the DynamoDB request dispatcher — the emulator must refuse a request addressed to a region EC2 DescribeRegions reports as not opted in, rather than serving it from state.
Traces: ddb-ltr-list-tables-in-a-not-opted-in-region-rejected.

## Evidence
ddb-ltr-list-tables-in-a-not-opted-in-region-rejected: AssertionError: the call succeeded; real AWS rejects it. The check's helper raises exactly this when the call it expected to be refused returns a response, so the emulator produced a successful ListTables for the non-opted-in region and there is no error code or message to compare.

## Steps to Reproduce
Replay the behavioral suite and re-run dynamodb__ec2.ddb-ltr-list-tables-in-a-not-opted-in-region-rejected. Minimal reproduction: call ec2.describe_regions(AllRegions=True) to find a region whose OptInStatus is 'not-opted-in', then call dynamodb.list_tables() against that region's endpoint — expect a refusal, observe a successful table list.
