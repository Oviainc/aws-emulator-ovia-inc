Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.parallel-scan-segments-partition-the-table-exactly-once
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.parallel-scan-segments-partition-the-table-exactly-once|run#25|AssertionError|assert err.get('Message') == expected, (
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# dynamodb: Scan's Segment/TotalSegments refusal message appends an implementation diagnostic to AWS's fixed text (1 cause)

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Scan

## Description
Cause 1 of 1: the ValidationException message for a Scan whose Segment is not less than TotalSegments is not AWS's text — the emulator appends a colon and a rendering of the two request values.

AWS: Scan refuses a parallel-scan request whose Segment is out of range with ValidationException (HTTP 400) and the fixed message text "The Segment parameter is zero-based and must be less than parameter TotalSegments". The message is entirely fixed text — it names the two member names and nothing else; it interpolates no request values. Smithy: ScanInput.Segment documents "The value for Segment must be greater than or equal to 0, and less than the value provided for TotalSegments"; the refusal is modeled as ValidationException on Scan.

Emulator: the error code and status are right but the message carries a trailing diagnostic: 'The Segment parameter is zero-based and must be less than parameter TotalSegments: Segment: 8 is not less than TotalSegments: 8'. The fixed part up to "TotalSegments" matches AWS; everything from the ":" onward (": Segment: <segment> is not less than TotalSegments: <totalSegments>", with both numbers taken from the request) is invented and must not be emitted.

Why: clients and test suites match DynamoDB validation messages exactly — the SDKs surface the message verbatim, and parallel-scan worker frameworks assert on it to distinguish a mis-partitioned worker from any other ValidationException. An appended value dump breaks equality matching, and leaking the request values into the message is a different contract from AWS's (it also changes with every request, so no client can pin it).

Fix: the Scan handler's Segment/TotalSegments validation in the dynamodb crate — emit AWS's fixed sentence only, with no appended values.

Traces: dynamodb.parallel-scan-segments-partition-the-table-exactly-once

## Evidence
Request: Scan(TableName=<table>, TotalSegments=8, Segment=8).

Expected (AWS): HTTP 400, code ValidationException, Message exactly:
  The Segment parameter is zero-based and must be less than parameter TotalSegments
(all fixed text; no request values interpolated).

Actual (emulator): same status and code, Message:
  The Segment parameter is zero-based and must be less than parameter TotalSegments: Segment: 8 is not less than TotalSegments: 8

Harness assertion:
  File "check_dynamodb_parallel_scan_segments_partition_the_table_exactly_once.py", line 157, in run
  AssertionError: expected message 'The Segment parameter is zero-based and must be less than parameter TotalSegments', got 'The Segment parameter is zero-based and must be less than parameter TotalSegments: Segment: 8 is not less than TotalSegments: 8'

The "8"s are this request's TotalSegments/Segment values — they fill the emulator's invented suffix and are not part of any AWS template.

## Steps to Reproduce
1. CreateTable with a simple hash key and put a few items; wait for ACTIVE.
2. Call Scan(TableName=<table>, TotalSegments=N, Segment=N) (Segment equal to TotalSegments; any Segment >= TotalSegments, or a negative Segment, triggers the same rule).
3. Read the ValidationException Message.
Observed: AWS's sentence followed by ": Segment: <n> is not less than TotalSegments: <n>". Expected: AWS's sentence alone.
